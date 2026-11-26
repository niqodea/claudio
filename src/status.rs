//! The status the user asks for. It is a table whose columns line up: the label, a number
//! right-aligned, what the number is, and when that changes, or a text left-aligned. Its groups,
//! set apart by an empty line, each hold one kind of value, so each aligns as its kind does:
//!
//! - how the session runs, as text: the model, its effort and fast mode, the permission mode,
//!   which says what Claude does without asking for approval, what pays, and the id to resume
//! - the plan's usage limits, as numbers
//! - the context, as numbers: its tokens and its cache, and what is left before it compacts
//! - Claude Code, as text: its version and its MCP servers
//!
//! What only a command shows whole stays out: `:context` breaks the context down, and `:mcp`
//! tells why a server is not connected. Color marks only what needs the user's attention.
//!
//! The status shows the session as it stands, from the first moment:
//! - every fact comes from an answer Claude Code gives as it starts, before the first message:
//!   the effort as `get_settings` applies it, and the context's tokens as Claude Code counts them
//!   before the first call. The init that comes with the first message is too late, and in
//!   claudio's sessions it carries no effort
//! - what Claude Code tells only when asked, the MCP servers, the open status asks for again
//!   every second: Claude Code lists them only once it loaded them, a second or so after it
//!   starts, and sends no message as they connect
//! - the MCP row counts the servers of the session: those the user set up, and the claude.ai
//!   connectors that run or failed. claude.ai offers every account its connectors, which wait
//!   for a sign-in until the user sets one up, so those are offers, which would bury the
//!   servers the user knows of
//!
//! Each of these requests, Claude Code answers from what it holds, with no network traffic.
//! Ref: <https://www.nngroup.com/articles/gestalt-proximity/>, on grouping by space alone
//! Ref: <https://data.europa.eu/apps/data-visualisation-guide/table-design-scannability>, on
//! aligning text left and numbers right
//! Ref: <https://developers.openai.com/codex/agent-approvals-security>, whose approval policy
//! names what the permission mode decides
//! Ref: <https://clig.dev/#output>, on using color with intention

use std::num::NonZeroUsize;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::context::{COLD_COLOR, Context};
use crate::cut::{self, ScreenRow};
use crate::limits::{Limits, level, limit_level};
use crate::protocol::{Ceiling, Limit, McpServerStatus};
use crate::style::{ATTENTION, FAILED_OR_REMOVED, PLAIN, SUBDUED, Style};
use crate::text::{Line, Span};

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

/// What Claude Code tells of how the session runs and of itself, each none until it tells it.
#[derive(Default)]
pub struct Facts {
    pub model: Option<String>,
    pub effort: Option<String>,
    pub fast_mode: Option<String>,
    pub permission_mode: Option<String>,
    /// The plan of the account, which pays unless a key does.
    pub plan: Option<String>,
    /// The key that pays in place of the plan.
    pub paying_key: Option<String>,
    pub mcp_servers: Option<Vec<McpServerStatus>>,
}

/// The status's lines at `now`, which is `wall_now` on the wall clock. What Claude Code has not
/// told yet shows as `?`, never as a value of claudio's own: each fact comes from an answer
/// Claude Code gives before the first message, since the first message's init comes late, and
/// leaves out the effort in claudio's sessions. A new session has nothing to resume before its
/// first message, which starts it.
pub fn lines(
    facts: &Facts,
    session_id: Option<&str>,
    version: Option<&str>,
    limits: &Limits,
    context: &Context,
    now: Instant,
    wall_now: SystemTime,
) -> Vec<Line> {
    vec![
        text_row("Model", known(facts.model.as_deref())),
        text_row("Effort", known(facts.effort.as_deref())),
        text_row("Fast", known(facts.fast_mode.as_deref())),
        text_row(
            "Approval",
            known(facts.permission_mode.as_deref().map(permission_mode)),
        ),
        text_row("Pays", paying(facts)),
        text_row(
            "Resume",
            known(session_id.or(context.is_new().then_some("none yet"))),
        ),
        Line::default(),
        limit_row("Session", limits.session(wall_now), wall_now),
        limit_row("Week", limits.week(wall_now), wall_now),
        Line::default(),
        context_row(context, now, wall_now),
        ceiling_row(context),
        Line::default(),
        text_row("Version", known(version)),
        text_row("MCP", mcp(facts.mcp_servers.as_deref())),
    ]
}

/// The status's `lines` as the rows of a screen `columns` wide and `rows` tall: fitted to its
/// height, and each cut to its width, so the status takes exactly its rows on any screen.
pub fn screen_rows(lines: &[Line], columns: NonZeroUsize, rows: usize) -> Vec<ScreenRow> {
    (cut::fit_lines(lines, rows).iter())
        .map(|line| ScreenRow::cut(line, columns))
        .collect()
}

fn text_row(name: &str, value: Line) -> Line {
    Line::from(label(name)).with_line(value)
}

fn known(value: Option<&str>) -> Line {
    Line::from(value.unwrap_or("?"))
}

/// The permission mode in the words Claude Code shows it in.
/// Ref: `PermissionMode` in `sdk.d.ts`
fn permission_mode(mode: &str) -> &str {
    match mode {
        "acceptEdits" => "accept edits",
        "bypassPermissions" => "bypass permissions",
        "dontAsk" => "don't ask",
        mode => mode,
    }
}

/// The key that pays in place of the plan, in the color of what the user should know, as the
/// warning as claudio starts says, or else the plan.
fn paying(facts: &Facts) -> Line {
    match (&facts.paying_key, &facts.plan) {
        (Some(key), _) => Line::from(Span::new(key.as_str(), ATTENTION)),
        (None, plan) => known(plan.as_deref()),
    }
}

/// The states of a server that is not connected, as Claude Code names them, then in the
/// status's words, for one server and for more, first those that need the user: in red when they
/// failed, and in orange when they wait for the user to sign in.
/// Ref: `McpServerStatus` in `sdk.d.ts`
const NOT_CONNECTED: [(&str, &str, &str, Style); 4] = [
    ("failed", "failed", "failed", FAILED_OR_REMOVED),
    ("needs-auth", "needs sign-in", "need sign-in", ATTENTION),
    ("pending", "connecting", "connecting", PLAIN),
    ("disabled", "disabled", "disabled", PLAIN),
];

/// How many of the session's MCP servers are connected, then how many stand in each other state,
/// so the row's length follows the states, few, rather than the servers, which `:mcp` names. The
/// session's servers are those the user set up, and the claude.ai connectors that run or failed:
/// claude.ai offers every account its connectors, which wait for a sign-in until the user sets
/// one up, so those are offers rather than servers of the session.
fn mcp(servers: Option<&[McpServerStatus]>) -> Line {
    let Some(servers) = servers else {
        return known(None);
    };
    let is_offer = |server: &&McpServerStatus| {
        server.scope.as_deref() == Some("claudeai") && server.status == "needs-auth"
    };
    let servers: Vec<&McpServerStatus> = servers.iter().filter(|s| !is_offer(s)).collect();
    let count = |status: &str| (servers.iter()).filter(|s| s.status == status).count();
    let mut line = Line::from(match (servers.len(), count("connected")) {
        (0, _) => "none".to_string(),
        (all, connected) if connected == all => format!("{all} connected"),
        (all, connected) => format!("{connected} of {all} connected"),
    });
    for (status, one, more, style) in NOT_CONNECTED {
        let state = match count(status) {
            0 => continue,
            1 => format!("1 {one}"),
            many => format!("{many} {more}"),
        };
        line.push(" · ");
        line.push(Span::new(state, style));
    }
    line
}

fn limit_row(name: &str, limit: Option<Limit>, wall_now: SystemTime) -> Line {
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
fn context_row(context: &Context, now: Instant, wall_now: SystemTime) -> Line {
    let tokens = Cell::new(context.tokens().map_or("?".to_string(), grouped), None);
    match context.warm_until(now) {
        Some(cold_at) => {
            let time_left = cold_at - now;
            let color = (time_left <= NEAR_COLD).then_some(COLD_COLOR);
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
fn ceiling_row(context: &Context) -> Line {
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

fn unknown_row(name: &str) -> Line {
    row(name, &Cell::new("?", None), None, None)
}

/// A row of a number and what it is, followed by when that changes.
fn row(name: &str, number: &Cell, word: Option<&Cell>, change: Option<&Cell>) -> Line {
    let number = Cell::new(format!("{:>NUMBER_WIDTH$}", number.text), number.color);
    let start = Line::from([label(name), number.painted()]);
    match (word, change) {
        (word, Some(change)) => {
            let (text, color) = word.map_or(("", None), |word| (word.text.as_str(), word.color));
            let word = Cell::new(format!("{text:<WORD_WIDTH$}"), color);
            start
                .with("  ")
                .with(word.painted())
                .with("  ")
                .with(change.painted())
        }
        (Some(word), None) => start.with("  ").with(word.painted()),
        (None, None) => start,
    }
}

/// When something changes, like "resets at 16:00", in `color`.
fn change(verb: &str, at: SystemTime, wall_now: SystemTime, color: Option<Style>) -> Cell {
    Cell::new(
        format!("{verb:>VERB_WIDTH$} {}", clock(at, wall_now)),
        color,
    )
}

fn label(text: &str) -> Span {
    Span::new(format!("{text:<LABEL_WIDTH$}"), SUBDUED)
}

/// A part of a row, in its color, none for the terminal's own.
struct Cell {
    text: String,
    color: Option<Style>,
}

impl Cell {
    fn new(text: impl Into<String>, color: Option<Style>) -> Self {
        Cell {
            text: text.into(),
            color,
        }
    }

    fn painted(&self) -> Span {
        Span::new(self.text.clone(), self.color.unwrap_or(PLAIN))
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
    use crate::limits::Source;
    use crate::protocol::Usage;

    const HOUR: Duration = Duration::from_hours(1);

    fn facts() -> Facts {
        Facts {
            model: Some("claude-opus-5-5".to_string()),
            effort: Some("high".to_string()),
            fast_mode: Some("off".to_string()),
            permission_mode: Some("acceptEdits".to_string()),
            plan: Some("Claude Team".to_string()),
            paying_key: None,
            mcp_servers: Some(vec![]),
        }
    }

    /// The lines of a status, without their styles.
    fn plain_lines(
        context: &Context,
        limits: &Limits,
        now: Instant,
        wall_now: SystemTime,
    ) -> Vec<String> {
        lines(
            &facts(),
            Some("a1"),
            Some("1.2.3"),
            limits,
            context,
            now,
            wall_now,
        )
        .iter()
        .map(Line::text)
        .collect()
    }

    /// The line of `name` in the status of `facts`.
    fn row_of(facts: &Facts, name: &str) -> Line {
        let status = lines(
            facts,
            None,
            None,
            &Limits::new(),
            &Context::new(),
            Instant::now(),
            SystemTime::now(),
        );
        status
            .into_iter()
            .find(|line| line.text().starts_with(name))
            .expect("the row")
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
            Source::Reply {
                arrived_at: wall_now,
            },
        );
        let context = context_with(41_203, HOUR, 967_000, now);
        let in_an_hour = clock(wall_now + HOUR, wall_now);
        let in_four_days = clock(wall_now + 4 * 24 * HOUR, wall_now);
        assert_eq!(
            plain_lines(&context, &limits, now, wall_now),
            [
                "Model    claude-opus-5-5".to_string(),
                "Effort   high".to_string(),
                "Fast     off".to_string(),
                "Approval accept edits".to_string(),
                "Pays     Claude Team".to_string(),
                "Resume   a1".to_string(),
                String::new(),
                format!("Session        62%  used  resets at {in_an_hour}"),
                format!("Week           18%  used  resets at {in_four_days}"),
                String::new(),
                format!("Context     41,203  warm    cold at {in_an_hour}"),
                "Compact    925,797  left".to_string(),
                String::new(),
                "Version  1.2.3".to_string(),
                "MCP      none".to_string(),
            ]
        );
    }

    #[test]
    fn what_claude_code_has_not_told_shows_as_unknown() {
        let status = lines(
            &Facts::default(),
            None,
            None,
            &Limits::new(),
            &Context::new(),
            Instant::now(),
            SystemTime::now(),
        );
        assert_eq!(
            status.iter().map(Line::text).collect::<Vec<_>>(),
            [
                "Model    ?",
                "Effort   ?",
                "Fast     ?",
                "Approval ?",
                "Pays     ?",
                "Resume   ?",
                "",
                "Session          ?",
                "Week             ?",
                "",
                "Context          ?",
                "Compact          ?",
                "",
                "Version  ?",
                "MCP      ?",
            ]
        );
    }

    #[test]
    fn a_key_that_pays_shows_in_place_of_the_plan_in_orange() {
        let facts = Facts {
            paying_key: Some("ANTHROPIC_API_KEY".to_string()),
            ..facts()
        };
        let row = row_of(&facts, "Pays");
        assert_eq!(row.text(), "Pays     ANTHROPIC_API_KEY");
        assert_eq!(row.style_of("ANTHROPIC_API_KEY"), Some(ATTENTION));
    }

    #[test]
    fn the_mcp_servers_not_connected_are_counted_by_state() {
        let server = |name: &str, status: &str| McpServerStatus {
            name: name.to_string(),
            status: status.to_string(),
            scope: Some("user".to_string()),
        };
        let facts = Facts {
            mcp_servers: Some(vec![
                server("docs", "pending"),
                server("slack", "needs-auth"),
                server("github", "failed"),
                server("linear", "needs-auth"),
                server("notion", "connected"),
            ]),
            ..facts()
        };
        let row = row_of(&facts, "MCP");
        assert_eq!(
            row.text(),
            "MCP      1 of 5 connected · 1 failed · 2 need sign-in · 1 connecting"
        );
        assert_eq!(row.style_of("1 failed"), Some(FAILED_OR_REMOVED));
        assert_eq!(row.style_of("2 need sign-in"), Some(ATTENTION));
    }

    #[test]
    fn a_status_wider_and_taller_than_the_screen_takes_exactly_its_rows() {
        let status = lines(
            &facts(),
            Some("a1"),
            None,
            &Limits::new(),
            &Context::new(),
            Instant::now(),
            SystemTime::now(),
        );
        let columns = NonZeroUsize::new(12).expect("12 is not zero");
        let rows = screen_rows(&status, columns, 7);
        let texts: Vec<String> = rows.iter().map(|row| row.line().text()).collect();
        assert_eq!(texts.len(), 7);
        assert!(texts.iter().all(|text| text.chars().count() <= 12));
        assert_eq!(texts[0], "Model    cl…");
        assert_eq!(texts[6], "… 6 more ro…");
    }

    /// Each row counts what it shows rather than listing it, so the servers' names take no room,
    /// and a thousand servers in every state show the same rows, as wide, whatever their names.
    #[test]
    fn the_rows_count_the_mcp_servers_rather_than_list_them() {
        let widths = |name: &str| {
            let servers = (0..1000)
                .map(|n| McpServerStatus {
                    name: format!("{name} {n}"),
                    status: ["connected", "failed", "needs-auth", "pending", "disabled"][n % 5]
                        .to_string(),
                    scope: Some("user".to_string()),
                })
                .collect();
            let facts = Facts {
                mcp_servers: Some(servers),
                ..facts()
            };
            (lines(
                &facts,
                None,
                None,
                &Limits::new(),
                &Context::new(),
                Instant::now(),
                SystemTime::now(),
            )
            .iter())
            .map(Line::width)
            .collect::<Vec<_>>()
        };
        assert_eq!(widths("x"), widths("a server with a long name"));
    }

    #[test]
    fn claude_ai_connectors_that_wait_for_a_sign_in_are_offers_left_out() {
        let connector = |name: &str, status: &str| McpServerStatus {
            name: name.to_string(),
            status: status.to_string(),
            scope: Some("claudeai".to_string()),
        };
        let facts = Facts {
            mcp_servers: Some(vec![
                connector("claude.ai Gmail", "needs-auth"),
                connector("claude.ai Meetings", "connected"),
                connector("claude.ai Slack", "needs-auth"),
                connector("claude.ai Linear", "failed"),
            ]),
            ..facts()
        };
        assert_eq!(
            row_of(&facts, "MCP").text(),
            "MCP      1 of 2 connected · 1 failed"
        );
    }

    #[test]
    fn a_cache_about_to_turn_cold_shows_its_time_in_yellow() {
        let (now, wall_now) = (Instant::now(), SystemTime::now());
        let context = context_with(41_203, NEAR_COLD, 967_000, now);
        let status = lines(
            &facts(),
            None,
            None,
            &Limits::new(),
            &context,
            now,
            wall_now,
        );
        let cold_at = clock(wall_now + NEAR_COLD, wall_now);
        assert_eq!(
            status[10].style_of(&format!("  cold at {cold_at}")),
            Some(COLD_COLOR)
        );
    }

    #[test]
    fn a_context_near_its_ceiling_shows_what_is_left_in_the_color_of_its_level() {
        let now = Instant::now();
        let context = context_with(900_000, HOUR, 967_000, now);
        let status = lines(
            &facts(),
            None,
            None,
            &Limits::new(),
            &context,
            now,
            SystemTime::now(),
        );
        let color = level(900_000.0 / 967_000.0, false)
            .color()
            .expect("a level near its end has a color");
        assert_eq!(status[11].style_of("   67,000"), Some(color));
    }
}
