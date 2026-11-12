//! The status the user asks for: the model and the session, the plan's usage limits, and the
//! context with its cache. It is a table whose columns line up: the label, a number
//! right-aligned, what the number is, and when that changes. Color marks only what needs the
//! user's attention.
//! Ref: <https://clig.dev/#output>, on using color with intention

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::context::{COLD_COLOR, Context};
use crate::limits::{Limits, level, limit_level};
use crate::protocol::{Ceiling, Limit};
use crate::style::{RESET, STATUS_COLOR, YELLOW};

/// How long before the cache turns cold its cold-at time shows in yellow.
const NEAR_COLD: Duration = Duration::from_mins(5);

const LABEL_WIDTH: usize = 9;
// Fits a million tokens, as 1,000,000
const NUMBER_WIDTH: usize = 9;
const WORD_WIDTH: usize = 4;
// Fits "resets at", so the "at"s line up
const VERB_WIDTH: usize = 9;

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// The status's lines at `now`, which is `wall_now` on the wall clock. What Claude Code has not
/// told yet shows as `?`. A new session has nothing to resume before its first message, which
/// starts it.
pub fn lines(
    model: Option<&str>,
    session_id: Option<&str>,
    limits: &Limits,
    context: &Context,
    now: Instant,
    wall_now: SystemTime,
) -> Vec<String> {
    vec![
        text_row("Model", model),
        text_row(
            "Resume",
            session_id.or(context.is_new().then_some("none yet")),
        ),
        String::new(),
        limit_row("Session", limits.session(), wall_now),
        limit_row("Week", limits.week(), wall_now),
        String::new(),
        context_row(context, now, wall_now),
        ceiling_row(context),
    ]
}

fn text_row(name: &str, value: Option<&str>) -> String {
    format!("{}{}", label(name), value.unwrap_or("?"))
}

fn limit_row(name: &str, limit: Option<Limit>, wall_now: SystemTime) -> String {
    let Some(limit) = limit else {
        return unknown_row(name);
    };
    let color = limit_level(limit).color();
    let percent = Cell::new(format!("{:.0}%", limit.utilization * 100.0), color);
    let resets = limit
        .resets_at
        .map(|at| change("resets at", at, wall_now, None));
    row(
        name,
        &percent,
        Some(&Cell::new("used", color)),
        resets.as_ref(),
    )
}

/// The context's tokens, and whether its cache is warm. A new session's cache holds nothing to
/// lose, so its row shows only the tokens.
fn context_row(context: &Context, now: Instant, wall_now: SystemTime) -> String {
    let tokens = Cell::new(context.tokens().map_or("?".to_string(), grouped), None);
    match context.warm_until(now) {
        Some(cold_at) => {
            let time_left = cold_at - now;
            let color = (time_left <= NEAR_COLD).then_some(YELLOW);
            let cold_at = change("cold at", wall_now + time_left, wall_now, color);
            row(
                "Context",
                &tokens,
                Some(&Cell::new("warm", None)),
                Some(&cold_at),
            )
        }
        // The status says what is known, and a context whose size is unknown has no known cache
        None if context.tokens().is_some() && context.is_cold(now) => row(
            "Context",
            &tokens,
            Some(&Cell::new("cold", Some(COLD_COLOR))),
            None,
        ),
        None => row("Context", &tokens, None, None),
    }
}

/// The tokens the context can take before its ceiling, in the color of how near it is.
fn ceiling_row(context: &Context) -> String {
    let (name, ceiling) = match context.ceiling() {
        Some(Ceiling::Compact(tokens)) => ("Compact", Some(tokens)),
        Some(Ceiling::Window(tokens)) => ("Window", Some(tokens)),
        None => ("Compact", None),
    };
    let Some((ceiling, tokens)) = ceiling.zip(context.tokens()) else {
        return unknown_row(name);
    };
    let tokens_left = ceiling.saturating_sub(tokens);
    let color = level(to_f64(tokens) / to_f64(ceiling), tokens_left == 0).color();
    row(
        name,
        &Cell::new(grouped(tokens_left), color),
        Some(&Cell::new("left", color)),
        None,
    )
}

fn unknown_row(name: &str) -> String {
    row(name, &Cell::new("?", None), None, None)
}

/// A row of a number and what it is, followed by when that changes. Each column is padded
/// before its color, so the colors' codes take no width.
fn row(name: &str, number: &Cell, word: Option<&Cell>, change: Option<&Cell>) -> String {
    let number = Cell::new(format!("{:>NUMBER_WIDTH$}", number.text), number.color);
    let start = format!("{}{}", label(name), number.painted());
    match (word, change) {
        (word, Some(change)) => {
            let (text, color) = word.map_or(("", None), |word| (word.text.as_str(), word.color));
            let word = Cell::new(format!("{text:<WORD_WIDTH$}"), color);
            format!("{start}  {}  {}", word.painted(), change.painted())
        }
        (Some(word), None) => format!("{start}  {}", word.painted()),
        (None, None) => start,
    }
}

/// When something changes, like "resets at 16:00", in `color`.
fn change(verb: &str, at: SystemTime, wall_now: SystemTime, color: Option<&'static str>) -> Cell {
    Cell::new(
        format!("{verb:>VERB_WIDTH$} {}", clock(at, wall_now)),
        color,
    )
}

fn label(text: &str) -> String {
    format!("{STATUS_COLOR}{text:<LABEL_WIDTH$}{RESET}")
}

/// A part of a row, in its color, none for the terminal's own.
struct Cell {
    text: String,
    color: Option<&'static str>,
}

impl Cell {
    fn new(text: impl Into<String>, color: Option<&'static str>) -> Self {
        Cell {
            text: text.into(),
            color,
        }
    }

    fn painted(&self) -> String {
        match self.color {
            Some(color) => format!("{color}{}{RESET}", self.text),
            None => self.text.clone(),
        }
    }
}

/// A number with its thousands set apart by commas, like 41,203.
pub fn grouped(number: u64) -> String {
    let digits = number.to_string();
    let mut text = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            text.push(',');
        }
        text.push(digit);
    }
    text
}

/// The local time of `at`, to the minute it falls in, followed by its day when that is not the
/// day of `wall_now`. The status is a snapshot, which stays on screen while time passes, where a
/// clock time stays true and a countdown goes wrong.
/// Ref: <https://design.gitlab.com/content/date-and-time>, on absolute times for what users act on
fn clock(at: SystemTime, wall_now: SystemTime) -> String {
    let (at, now) = (to_local_time(at), to_local_time(wall_now));
    let time = format!("{:02}:{:02}", at.tm_hour, at.tm_min);
    if (at.tm_year, at.tm_yday) == (now.tm_year, now.tm_yday) {
        return time;
    }
    let month = MONTHS[usize::try_from(at.tm_mon).expect("tm_mon is 0 to 11")];
    format!("{time} on {:02} {month}", at.tm_mday)
}

/// The fields of `time` in the local time zone. The times come from Claude Code and the clock,
/// after 1970 and within `time_t`. A time before 1970 reads as 1970, and one past `time_t` as
/// its last second.
fn to_local_time(time: SystemTime) -> libc::tm {
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let seconds = libc::time_t::try_from(seconds).unwrap_or(libc::time_t::MAX);
    // SAFETY: tm is plain data, for which zero is a valid value
    let mut fields: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers are valid for the call, which leaves the fields at zero when it fails
    unsafe { libc::localtime_r(&raw const seconds, &raw mut fields) };
    fields
}

/// A count of tokens as a number for a share. A context holds far fewer than 4,294,967,295
/// tokens, and a larger count reads as that many.
fn to_f64(tokens: u64) -> f64 {
    f64::from(u32::try_from(tokens).unwrap_or(u32::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Usage;

    const HOUR: Duration = Duration::from_hours(1);

    /// The lines of a status, without their colors' codes.
    fn plain_lines(
        context: &Context,
        limits: &Limits,
        now: Instant,
        wall_now: SystemTime,
    ) -> Vec<String> {
        lines(
            Some("claude-opus-5-5"),
            Some("a1"),
            limits,
            context,
            now,
            wall_now,
        )
        .iter()
        .map(|line| strip_colors(line))
        .collect()
    }

    fn strip_colors(line: &str) -> String {
        let mut text = String::new();
        let mut chars = line.chars();
        while let Some(character) = chars.next() {
            if character == '\x1b' {
                chars.by_ref().find(|&end| end == 'm');
            } else {
                text.push(character);
            }
        }
        text
    }

    fn context_with(tokens: u64, cache_ttl: Duration, ceiling: u64, now: Instant) -> Context {
        let mut context = Context::new();
        context.note_call(
            &Usage {
                tokens,
                cache_ttl: Some(cache_ttl),
            },
            now,
        );
        context.note_ceiling(Ceiling::Compact(ceiling));
        context
    }

    #[test]
    fn the_columns_line_up_and_the_ats_too() {
        let (now, wall_now) = (Instant::now(), SystemTime::now());
        let mut limits = Limits::new();
        limits.note(
            Some(Limit {
                utilization: 0.62,
                is_reached: false,
                resets_at: Some(wall_now + HOUR),
            }),
            Some(Limit {
                utilization: 0.18,
                is_reached: false,
                resets_at: Some(wall_now + 4 * 24 * HOUR),
            }),
        );
        let context = context_with(41_203, HOUR, 967_000, now);
        let in_an_hour = clock(wall_now + HOUR, wall_now);
        let in_four_days = clock(wall_now + 4 * 24 * HOUR, wall_now);
        assert_eq!(
            plain_lines(&context, &limits, now, wall_now),
            [
                "Model    claude-opus-5-5".to_string(),
                "Resume   a1".to_string(),
                String::new(),
                format!("Session        62%  used  resets at {in_an_hour}"),
                format!("Week           18%  used  resets at {in_four_days}"),
                String::new(),
                format!("Context     41,203  warm    cold at {in_an_hour}"),
                "Compact    925,797  left".to_string(),
            ]
        );
    }

    #[test]
    fn what_claude_code_has_not_told_shows_as_unknown() {
        let status = lines(
            None,
            None,
            &Limits::new(),
            &Context::new(),
            Instant::now(),
            SystemTime::now(),
        );
        assert_eq!(
            status
                .iter()
                .map(|line| strip_colors(line))
                .collect::<Vec<_>>(),
            [
                "Model    ?",
                "Resume   ?",
                "",
                "Session          ?",
                "Week             ?",
                "",
                "Context          ?",
                "Compact          ?",
            ]
        );
    }

    #[test]
    fn a_cache_about_to_turn_cold_shows_its_time_in_yellow() {
        let (now, wall_now) = (Instant::now(), SystemTime::now());
        let context = context_with(41_203, NEAR_COLD, 967_000, now);
        let status = lines(None, None, &Limits::new(), &context, now, wall_now);
        let cold_at = clock(wall_now + NEAR_COLD, wall_now);
        assert!(status[6].contains(&format!("{YELLOW}  cold at {cold_at}")));
    }

    #[test]
    fn a_context_near_its_ceiling_shows_what_is_left_in_the_color_of_its_level() {
        let now = Instant::now();
        let context = context_with(900_000, HOUR, 967_000, now);
        let status = lines(None, None, &Limits::new(), &context, now, SystemTime::now());
        let color = level(900_000.0 / 967_000.0, false)
            .color()
            .expect("a level near its end has a color");
        assert!(status[7].contains(&format!("{color}   67,000")));
    }
}
