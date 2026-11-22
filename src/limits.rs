//! The plan's usage limits as the API counts them: how much of each the account used, over five
//! hours and over seven days. Each share comes from a [`Source`], which tells when it was true.
//! A limit is its newest share while that share is current, and unknown after.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use crate::protocol::{self, Limit, UsageCopy};

/// How often claudio asks Claude Code for the plan's usage, which keeps Claude Code's copy of it
/// current. Claude Code answers from the copy, which all its processes share, until the copy is a
/// minute old, and only then fetches the usage again, so each session adds at most one request to
/// the usage endpoint a minute, and sessions that ask together share one.
/// Ref: <https://github.com/jarrodwatts/claude-hud/issues/173>, on the usage endpoint answering a
/// status line with HTTP 429
///
/// Undocumented: [`crate::contract::the_usage_is_shared_for_a_minute`]
const ASK_EVERY: Duration = Duration::from_mins(1);

/// How often claudio reads Claude Code's copy of the plan's usage. Any Claude Code process can
/// renew the copy, so claudio reads it more often than it asks.
const READ_EVERY: Duration = Duration::from_secs(5);

/// How long a share of a limit stays current. Every session of the account adds to the limits,
/// so an older share can be short of the real one, and a share short of the real one cannot keep
/// the user under the limit.
///
/// Claude Code answers an ask from a copy just under a minute old without fetching again, and the
/// next ask comes [`ASK_EVERY`] later, so a copy can turn 2 minutes old before a fetch renews it.
/// The third minute covers that fetch and the [`READ_EVERY`] before claudio reads its copy.
const CURRENT_FOR: Duration = Duration::from_mins(3);

/// The shares of a limit that raise its level by one each. They are this client's own choice:
/// Claude Code's server decides where Claude Code warns, and no source lists those points. The
/// steps get smaller near the limit, where less usage remains. They hold for anything that fills
/// up to a limit, so one scale says how near each is to its end.
const LEVEL_BOUNDS: [f64; 3] = [0.5, 0.75, 0.9];

/// The number of bounds passed from which a level is near its end.
const NEAR_BOUNDS_PASSED: usize = 2;

use crate::style::{ATTENTION, FAILED_OR_REMOVED, Style};

/// How much of a limit the account used.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Level {
    /// Short of the limit, with how many of `LEVEL_BOUNDS` the account passed.
    Below(usize),
    Reached,
}

impl Level {
    /// The number the indicator shows: the bounds passed, then one more once reached.
    pub fn to_number(self) -> usize {
        match self {
            Level::Below(passed) => passed,
            Level::Reached => LEVEL_BOUNDS.len() + 1,
        }
    }

    /// Orange near the end, red once reached, none below that.
    pub fn color(self) -> Option<Style> {
        match self {
            Level::Below(passed) if passed < NEAR_BOUNDS_PASSED => None,
            Level::Below(_) => Some(ATTENTION),
            Level::Reached => Some(FAILED_OR_REMOVED),
        }
    }
}

pub struct Limits {
    session: Option<Told>,
    week: Option<Told>,
}

impl Limits {
    pub fn new() -> Self {
        Limits {
            session: None,
            week: None,
        }
    }

    /// Stores the limits from `source`. A limit it left out, or told as true before the stored
    /// one, keeps the stored one.
    pub fn note(&mut self, session: Option<Limit>, week: Option<Limit>, source: Source) {
        let at = source.true_at();
        self.session = newest(self.session, session, at);
        self.week = newest(self.week, week, at);
    }

    /// The limit over five hours, none while no share of it is current at `now`.
    pub fn session(&self, now: SystemTime) -> Option<Limit> {
        current(self.session, now)
    }

    /// The limit over seven days, none while no share of it is current at `now`.
    pub fn week(&self, now: SystemTime) -> Option<Limit> {
        current(self.week, now)
    }
}

/// Where a share comes from, with when it was true. No source tells when Anthropic measured the
/// share, so the time is when it reached this machine, by this machine's clock.
///
/// Claude Code's answer to `get_usage` is no source: its share can be an old one with nothing to
/// tell so, as when the usage endpoint fails.
#[derive(Clone, Copy)]
pub enum Source {
    /// A reply of Claude's, which carries the share the API counted as it answered.
    Reply { arrived_at: SystemTime },
    /// Claude Code's copy of its last fetch from the usage endpoint, in its config file.
    Copy { fetched_at: SystemTime },
}

impl Source {
    fn true_at(self) -> SystemTime {
        match self {
            Source::Reply { arrived_at } => arrived_at,
            Source::Copy { fetched_at } => fetched_at,
        }
    }
}

/// Keeps the limits current in the background, for as long as `ask` succeeds: asks Claude Code
/// for the plan's usage with `ask` every [`ASK_EVERY`], which renews Claude Code's copy of it,
/// and hands the copy to `note` every [`READ_EVERY`]. Without a config directory to read the copy
/// from, it does nothing.
pub fn keep_current(
    ask: impl Fn() -> Result<(), String> + Send + 'static,
    note: impl Fn(Option<Limit>, Option<Limit>, Source) + Send + 'static,
) {
    let Some(config) = config_path() else {
        return;
    };
    thread::spawn(move || {
        let mut asked_at: Option<Instant> = None;
        loop {
            if asked_at.is_none_or(|at| at.elapsed() >= ASK_EVERY) {
                if ask().is_err() {
                    return;
                }
                asked_at = Some(Instant::now());
            }
            let copy = read_copy(&config);
            if let Some(copy) = copy {
                note(
                    copy.session,
                    copy.week,
                    Source::Copy {
                        fetched_at: copy.fetched_at,
                    },
                );
            }
            thread::sleep(READ_EVERY);
        }
    });
}

/// A limit, with when its share was true.
#[derive(Clone, Copy)]
struct Told {
    limit: Limit,
    at: SystemTime,
}

/// The newer of `stored` and `limit`, which was true at `at`, or `stored` when `limit` is none.
fn newest(stored: Option<Told>, limit: Option<Limit>, at: SystemTime) -> Option<Told> {
    match limit {
        Some(limit) if stored.is_none_or(|told| told.at <= at) => Some(Told { limit, at }),
        _ => stored,
    }
}

fn current(told: Option<Told>, now: SystemTime) -> Option<Limit> {
    // A share true after `now`, like a fetch saved just after `now` was taken, is current
    let is_current = |told: &Told| now.duration_since(told.at).unwrap_or_default() < CURRENT_FOR;
    told.filter(is_current).map(|told| told.limit)
}

/// Claude Code's config file: `.claude.json` in `$CLAUDE_CONFIG_DIR`, or in the home directory.
/// Where a legacy `.config.json` is left in Claude Code's config directory, Claude Code can use it
/// instead, and the limits then show as unknown.
pub(crate) fn config_path() -> Option<PathBuf> {
    let directory = (env::var_os("CLAUDE_CONFIG_DIR").filter(|directory| !directory.is_empty()))
        .or_else(|| env::var_os("HOME"))?;
    Some(PathBuf::from(directory).join(".claude.json"))
}

/// Claude Code's copy of its last fetch of the plan's usage, from its config file at `config`,
/// none when the file holds no readable copy.
pub(crate) fn read_copy(config: &Path) -> Option<UsageCopy> {
    let text = fs::read_to_string(config).ok()?;
    protocol::read_usage_copy(&text)
}

/// The level of a limit by how much of it the account used.
pub fn limit_level(limit: Limit) -> Level {
    level(limit.utilization, limit.is_reached)
}

/// The level of `share`, a share of a limit from 0 to 1, with `is_reached` telling whether the
/// limit is reached.
pub fn level(share: f64, is_reached: bool) -> Level {
    if is_reached {
        return Level::Reached;
    }
    let passed = LEVEL_BOUNDS
        .iter()
        .take_while(|&&bound| share >= bound)
        .count();
    Level::Below(passed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(arrived_at: SystemTime) -> Source {
        Source::Reply { arrived_at }
    }

    fn used(utilization: f64) -> Limit {
        Limit {
            utilization,
            is_reached: false,
            resets_at: None,
        }
    }

    #[test]
    fn each_bound_passed_raises_the_level() {
        assert_eq!(limit_level(used(0.0)), Level::Below(0));
        assert_eq!(limit_level(used(0.49)), Level::Below(0));
        assert_eq!(limit_level(used(0.5)), Level::Below(1));
        assert_eq!(limit_level(used(0.75)), Level::Below(2));
        assert_eq!(limit_level(used(0.9)), Level::Below(3));
        assert_eq!(limit_level(used(0.99)), Level::Below(3));
    }

    #[test]
    fn a_reached_limit_shows_one_past_the_last_bound() {
        let reached = Limit {
            utilization: 1.0,
            is_reached: true,
            resets_at: None,
        };
        assert_eq!(limit_level(reached), Level::Reached);
        assert_eq!(Level::Reached.to_number(), 4);
    }

    #[test]
    fn a_limit_left_untold_keeps_its_level() {
        let now = SystemTime::now();
        let mut limits = Limits::new();
        limits.note(Some(used(0.6)), Some(used(0.2)), reply(now));
        limits.note(None, Some(used(0.8)), reply(now));
        assert_eq!(limits.session(now).map(limit_level), Some(Level::Below(1)));
        assert_eq!(limits.week(now).map(limit_level), Some(Level::Below(2)));
    }

    #[test]
    fn a_share_true_before_the_stored_one_is_dropped() {
        let now = SystemTime::now();
        let mut limits = Limits::new();
        limits.note(Some(used(0.6)), None, reply(now));
        let fetched_at = now - Duration::from_secs(30);
        limits.note(Some(used(0.2)), None, Source::Copy { fetched_at });
        assert_eq!(limits.session(now), Some(used(0.6)));
    }

    #[test]
    fn a_limit_is_unknown_once_its_share_is_no_longer_current() {
        let at = SystemTime::now();
        let mut limits = Limits::new();
        limits.note(Some(used(0.6)), Some(used(0.2)), reply(at));
        assert_eq!(
            limits.session(at + CURRENT_FOR - Duration::from_secs(1)),
            Some(used(0.6))
        );
        assert_eq!(limits.session(at + CURRENT_FOR), None);
        assert_eq!(limits.week(at + CURRENT_FOR), None);
    }
}
