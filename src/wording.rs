//! The grammar of the conversation as claudio writes it: the marks that set apart what the user
//! and Claude write, which shows as written, and the lines claudio writes itself. This is the
//! one place the grammar is written out, and each item here gives the reason for its part, so
//! a change to the grammar changes its reason with it. The README's Wording section gives the
//! rules that the words of every line follow.
//!
//! ```text
//! conversation = (message │ user line │ answer │ decision │ reply │ detail line │ boundary)*
//! message      = "››" "\n" text                                your message, on the bar
//! user line    = "›› " what                                    a command, like `/compact`, on the bar
//! answer       = question " › " text                           your answer to a question, on the bar
//! decision     = verb " " part                                 like `allowed #01 Bash: Run the tests`, on the bar
//! reply        = "‹‹" [" summarized"] "\n" text                Claude's text, or the API's summary of it
//! detail line  = call line │ notice
//! call line    = ("○" │ "◌" │ "✓" │ "✗") " " body              a call, running, and how it ended
//! notice       = ("-" │ "!") " " header ("\n" quoted)*         noted, or needs the user
//! header       = body                                          claudio's words, one line
//! quoted       = a line from elsewhere, as written             a hook's output, Claude Code's words
//! body         = part (" · " part)* [" · " hint]
//! part         = [subject ": "] text
//! hint         = key " " verb [object]                         like `Ctrl+O shows it whole`
//!              │ verb " with " command                         like `resume with claudio --resume <id>`
//! boundary     = "─── " event " ───" [" · " hint]             where a session ends
//! label row    = group (" │ " group)*
//! ```
//!
//! The types keep the grammar by construction:
//!
//! - `marks`, `reply_mark`, `summarized_reply_mark`, `user_line` and `answer_label` write the
//!   marks of who speaks.
//! - A call's line takes a `CallMark`, which gives no color.
//! - A notice takes a `NoticeMark`, which only a `Notice` writes, and which gives the only color
//!   a mark gives.
//! - `Notice` takes its header from claudio and anything else as quoted text.
//! - `part`, `parts`, `Hint`, `command_hint` and `GROUP` join the pieces of a line.
//! - Every line here is as wide as its text, whatever the terminal's width, as the README's
//!   Design section asks.

use crate::style::{
    ATTENTION, CODE, FAILED_OR_REMOVED, MARK, PLAIN, STRONG, SUBDUED, SUBDUED_ON_BAR, Style, YOURS,
};
use crate::text::{Line, Row, Span};

/// Between the parts of a line, like a task's end and its count of calls, and before a hint.
pub const PART: &str = " · ";
/// Between the groups of the label row, which hold indicators of one kind each.
pub const GROUP: &str = " │ ";

/// The rule on each side of a boundary, of a fixed length.
const RULE: &str = "───";

/// The mark in front of a call's line, which says how far the call got, and leaves the line's
/// colors to the line. The marks are Primer's, but `○` and `◌`, and `✗` marks a failure as
/// Dynatrace marks a failed process. A failed call takes no color either: failing is part of
/// Claude's work, like a search that finds nothing, and Claude deals with it.
/// Ref: <https://github.com/primer/design/blob/main/content/native/cli/foundations.mdx#iconography>
/// Ref: <https://developer.dynatrace.com/design/status-and-health>
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallMark {
    /// A call Claude makes.
    Started,
    /// A call whose work goes on in the background.
    Running,
    /// A call or a task that succeeded.
    Succeeded,
    /// A call or a task that failed, which Claude deals with.
    Failed,
}

impl CallMark {
    #[cfg(test)]
    const ALL: [CallMark; 4] = [
        CallMark::Started,
        CallMark::Running,
        CallMark::Succeeded,
        CallMark::Failed,
    ];

    fn symbol(self) -> &'static str {
        match self {
            CallMark::Started => "○",
            CallMark::Running => "◌",
            CallMark::Succeeded => "✓",
            CallMark::Failed => "✗",
        }
    }

    fn style(self) -> Style {
        match self {
            CallMark::Started => STRONG,
            CallMark::Running | CallMark::Succeeded | CallMark::Failed => PLAIN,
        }
    }

    /// The line of `body` after the mark.
    pub fn line(self, body: impl Into<Line>) -> Line {
        Line::from([Span::new(self.symbol(), self.style()), " ".into()]).with_line(body.into())
    }
}

/// The mark in front of a notice, which says whether it needs the user, and gives the notice
/// its color, the only color a mark gives. `!` marks what needs the user, as Dynatrace marks
/// what needs attention at both its warning and critical levels, and only then does a notice
/// take a color: red for what failed and needs them, as Carbon's red signals "that user action
/// is needed immediately", and orange for what they should know. A notice that needs nothing of
/// them takes `-` and no color, as Carbon asks for plain text where no action is.
/// Ref: <https://developer.dynatrace.com/design/status-and-health>
/// Ref: <https://v10.carbondesignsystem.com/patterns/status-indicator-pattern/>
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeMark {
    /// A change the user may want to know of, which needs nothing of them, like a compaction.
    Noted,
    /// What the user should know, like a key that pays for the session.
    Warning,
    /// What failed and needs the user, like their hook.
    Error,
}

impl NoticeMark {
    #[cfg(test)]
    const ALL: [NoticeMark; 3] = [NoticeMark::Noted, NoticeMark::Warning, NoticeMark::Error];

    fn symbol(self) -> &'static str {
        match self {
            NoticeMark::Noted => "-",
            NoticeMark::Warning | NoticeMark::Error => "!",
        }
    }

    /// The style of the mark and of all the notice's text.
    fn style(self) -> Style {
        match self {
            NoticeMark::Noted => SUBDUED,
            NoticeMark::Warning => ATTENTION,
            NoticeMark::Error => FAILED_OR_REMOVED,
        }
    }
}

/// A notice: a header after its mark, in claudio's words, and the text it quotes, like a hook's
/// error output or what Claude Code says, under it as written. A notice is the only way to
/// write a line with a notice's mark.
///
/// The header names the source, like `Claude Code warns`, so the user always knows whose words
/// they read, and the quoted text takes no mark, indent or prefix, since claudio never rewrites
/// words it did not write. A notice of one line takes the same shape as one of many, so a
/// one-line warning of Claude Code's takes two lines.
/// Ref: <https://clig.dev/#errors>, which asks to group lines "under a single explanatory header"
pub struct Notice {
    mark: NoticeMark,
    header: Line,
    quoted: Option<String>,
}

impl Notice {
    /// A notice of one line, `header`, which claudio writes. The header's spans keep what sets
    /// them apart, like the bold of a command, on the mark's style.
    pub fn new(mark: NoticeMark, header: impl Into<Line>) -> Self {
        let header = header.into();
        debug_assert!(
            !header.text().contains('\n'),
            "a header is one line: {header}"
        );
        Notice {
            mark,
            header,
            quoted: None,
        }
    }

    /// The notice with `text` under its header, without the empty lines at its ends, which
    /// would set it apart from the header, and without any when it is empty.
    pub fn quoting(self, text: &str) -> Self {
        let text = text.trim_matches(|character: char| character == '\n' || character == '\r');
        Notice {
            quoted: (!text.trim().is_empty()).then(|| text.to_string()),
            ..self
        }
    }

    pub fn lines(self) -> Vec<Line> {
        let style = self.mark.style();
        let marked = Line::from([Span::new(self.mark.symbol(), style), " ".into()]);
        let quoted = self.quoted.iter().flat_map(|text| text.lines());
        std::iter::once(marked.with_line(self.header.over(style)))
            .chain(quoted.map(|line| Notice::more(self.mark, line)))
            .collect()
    }

    /// A line more of the text a notice of `mark` quotes, for one whose text arrives a line at a
    /// time, like Claude Code's error output, right under the notice's last line.
    ///
    /// The quoted text reads as its header's by standing right under it, and the next mark ends
    /// it, rather than a gutter or a box: nearness groups as strongly as anything but a boundary,
    /// and a boundary, like a `│` or an indent on every line, clutters, and breaks the text when
    /// copied, as users of Claude Code report of its indented output.
    /// Ref: <https://www.nngroup.com/articles/gestalt-proximity/>
    /// Ref: <https://www.nngroup.com/articles/common-region/>, which warns that overused
    /// boundaries clutter
    /// Ref: <https://github.com/anthropics/claude-code/issues/18170>
    pub fn more(mark: NoticeMark, line: &str) -> Line {
        Line::from(Span::new(line, mark.style()))
    }
}

/// A line of what the user did, like a command, on the bar, where the user's lines
/// are, in the style of the label row's state. Only the user's lines are on the bar.
pub fn user_line(what: &str) -> Row {
    Line::from([marks(), Span::new(format!(" {what}"), SUBDUED_ON_BAR)]).on(YOURS)
}

/// The marks in front of a message, on a row of their own. `reply_mark`'s `‹‹` starts what
/// Claude writes the same way and in the same color, since both say who starts: their characters
/// tell the user's lines from Claude's once the scrollback is copied, without colors.
pub fn marks() -> Span {
    Span::new("››", MARK)
}

/// The marks in front of what Claude writes, on a row of their own.
pub fn reply_mark() -> Line {
    Line::from(Span::new("‹‹", MARK))
}

/// The marks in front of a narration, which the API sends only as a summary of what Claude
/// wrote, on a row of their own, so the user knows they don't read Claude's exact words. The
/// word goes on the mark's row rather than after the text, where Claude Code's interface puts
/// ` · summarized`, so claudio's words stay off a line of words from elsewhere, as a `Notice`
/// keeps them. It is dim, since it says something about the text, which is what the user reads.
pub fn summarized_reply_mark() -> Line {
    Line::from([Span::new("‹‹", MARK), Span::new(" summarized", SUBDUED)])
}

/// The question in front of an answer.
pub fn answer_label(question: &str) -> Line {
    Line::from([
        Span::new(question, STRONG),
        " ".into(),
        Span::new("›", MARK),
        " ".into(),
    ])
}

/// What a key does, which the user can learn from where they look, like `Ctrl+C stops`.
pub struct Hint<'a> {
    /// The key's name, with a capital and + between the keys of a shortcut, like `Ctrl+O`.
    key: &'a str,
    /// What the key does, from a verb, in as few words as read well.
    does: &'a str,
}

impl<'a> Hint<'a> {
    pub const fn new(key: &'a str, does: &'a str) -> Self {
        Hint { key, does }
    }
}

impl std::fmt::Display for Hint<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{} {}", self.key, self.does)
    }
}

/// `parts` as one line's text, each set apart from the next by ` · `, a hint included, as
/// `part` sets a part's subject apart from what it says by `: `. No style guide read covers these
/// separators, so they follow Claude Code's own notices, like `Previous session saved · resume
/// with: claude --resume <id>`.
pub fn parts(parts: &[String]) -> String {
    parts.join(PART)
}

/// A part that says `text` of `subject`, like `#00 Bash: completed`.
pub fn part(subject: &str, text: &str) -> String {
    format!("{subject}: {text}")
}

/// `text`, then the hint of what the user can do about it.
pub fn with_hint(text: &str, hint: &Hint) -> String {
    format!("{text}{PART}{hint}")
}

/// The hints of several keys, as one part.
pub fn hints(hints: &[Hint]) -> String {
    parts(&hints.iter().map(ToString::to_string).collect::<Vec<_>>())
}

/// The hint that names `command` as the way to `verb`, like `resume with claudio --resume <id>`,
/// with the command set as code.
pub fn command_hint(verb: &str, command: &str) -> Line {
    Line::from([format!("{verb} with ").into(), Span::new(command, CODE)])
}

/// What happened to a session, which a boundary names.
#[derive(Clone, Copy)]
pub enum SessionEnd {
    /// The context was cleared, which starts a new session.
    Cleared,
    /// claudio ended.
    Ended,
}

/// The boundary that names how a session ended, then the command that resumes the session, when
/// the session's id is known. The rules on each side have a fixed length, so the line stays as
/// wide as its text, and they make the boundary stand out among the lines.
pub fn session_end(end: SessionEnd, session: Option<&str>) -> Line {
    let event = match end {
        SessionEnd::Cleared => "cleared",
        SessionEnd::Ended => "ended",
    };
    let boundary = Line::from([
        Span::new(RULE, MARK),
        " ".into(),
        Span::new(event, STRONG),
        " ".into(),
        Span::new(RULE, MARK),
    ]);
    match session {
        Some(session) => {
            let hint = command_hint("resume", &format!("claudio --resume {session}"));
            boundary.with_line(Line::from(PART).with_line(hint).over(SUBDUED))
        }
        None => boundary,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_what_needs_the_user_takes_a_color_and_it_says_so_with_its_mark() {
        let colors = [ATTENTION, FAILED_OR_REMOVED];
        for mark in NoticeMark::ALL {
            assert_eq!(
                colors.contains(&mark.style()),
                mark.symbol() == "!",
                "{mark:?}"
            );
        }
        for mark in CallMark::ALL {
            assert!(!colors.contains(&mark.style()), "{mark:?}");
        }
    }

    #[test]
    fn a_notice_quotes_text_as_written_under_its_header() {
        let notice = Notice::new(NoticeMark::Error, "Stop hook failed").quoting("\n  one\n\ntwo\n");
        let text: Vec<String> = notice.lines().iter().map(Line::text).collect();
        assert_eq!(text, ["! Stop hook failed", "  one", "", "two"]);
        let blank = Notice::new(NoticeMark::Noted, "compacted").quoting("\n \n");
        assert_eq!(blank.lines().len(), 1);
    }

    #[test]
    fn a_hint_follows_its_text_as_a_part() {
        let stop = Hint::new("Ctrl+C", "stops");
        let edit = Hint::new("Enter", "edits");
        assert_eq!(
            with_hint("clear waits", &stop),
            "clear waits · Ctrl+C stops"
        );
        assert_eq!(hints(&[stop, edit]), "Ctrl+C stops · Enter edits");
    }

    #[test]
    fn a_session_ends_with_its_boundary_then_how_to_resume_it() {
        let cleared = session_end(SessionEnd::Cleared, Some("a1"));
        assert_eq!(
            cleared.text(),
            "─── cleared ─── · resume with claudio --resume a1"
        );
        assert_eq!(cleared.style_of(" · resume with "), Some(SUBDUED));
        assert_eq!(
            cleared.style_of("claudio --resume a1"),
            Some(CODE.on(SUBDUED))
        );
        assert_eq!(session_end(SessionEnd::Ended, None).text(), "─── ended ───");
    }
}
