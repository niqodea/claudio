//! The plan's usage limits as the API counts them: how much of each the account used, over five
//! hours and over seven days. Claude Code sends them along with Claude's replies.

use crate::protocol::Limit;

/// The shares of a limit that raise its level by one each. They are this client's own choice:
/// Claude Code's server decides where Claude Code warns, and no source lists those points. The
/// steps get smaller near the limit, where less usage remains. They hold for anything that fills
/// up to a limit, so one scale says how near each is to its end.
const LEVEL_BOUNDS: [f64; 3] = [0.5, 0.75, 0.9];

/// The number of bounds passed from which a level is near its end.
const NEAR_BOUNDS_PASSED: usize = 2;

use crate::style::{ORANGE, RED};

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
    pub fn color(self) -> Option<&'static str> {
        match self {
            Level::Below(passed) if passed < NEAR_BOUNDS_PASSED => None,
            Level::Below(_) => Some(ORANGE),
            Level::Reached => Some(RED),
        }
    }
}

pub struct Limits {
    session: Option<Limit>,
    week: Option<Limit>,
}

impl Limits {
    pub fn new() -> Self {
        Limits {
            session: None,
            week: None,
        }
    }

    /// Stores the limits Claude Code sent. A limit it left out keeps the value it sent last.
    pub fn note(&mut self, session: Option<Limit>, week: Option<Limit>) {
        self.session = session.or(self.session);
        self.week = week.or(self.week);
    }

    /// The limit over five hours, none before Claude Code sends it.
    pub fn session(&self) -> Option<Limit> {
        self.session
    }

    /// The limit over seven days, none before Claude Code sends it.
    pub fn week(&self) -> Option<Limit> {
        self.week
    }
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
        let mut limits = Limits::new();
        limits.note(Some(used(0.6)), Some(used(0.2)));
        limits.note(None, Some(used(0.8)));
        assert_eq!(limits.session().map(limit_level), Some(Level::Below(1)));
        assert_eq!(limits.week().map(limit_level), Some(Level::Below(2)));
    }
}
