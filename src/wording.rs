//! The grammar of the conversation as claudio writes it: the marks that set apart what the user
//! and Claude write, which shows as written, and the lines claudio writes itself. `AGENTS.md`'s
//! Lines section gives each rule's source and reason.
//!
//! ```text
//! conversation = (message │ user line │ answer │ decision │ reply │ detail line │ boundary)*
//! message      = "››" "\n" text                                your message, on the bar
//! user line    = "›› " what                                    a command, like `/compact`, on the bar
//! answer       = question " › " text                           your answer to a question, on the bar
//! decision     = verb " " part                                 like `allowed #01 Bash: Run the tests`, on the bar
//! reply        = "‹‹" "\n" text                                Claude's text
//! detail line  = call line │ notice
//! call line    = ("○" │ "◌" │ "✓" │ "✗") " " body              a call, running, and how it ended
//! notice       = ("-" │ "!") " " header ("\n" quoted)*         noted, or needs the user
//! header       = body                                          claudio's words, one line
//! quoted       = a line from elsewhere, as written             a hook's output, Claude Code's words
//! body         = part (" · " part)* [" · " hint]
//! part         = [subject ": "] text
//! hint         = key " " verb [object]                         like `Ctrl+O shows it whole`
//!              │ verb " with " command                         like `resume with claudio --resume <id>`
//! boundary     = "─── " event " ───"                           where a session ends
//! label row    = group (" │ " group)*
//! ```
//!
//! The types keep the grammar by construction:
//!
//! - `marks`, `reply_mark`, `user_line` and `answer_label` write the marks of who speaks: `››`
//!   for the user, `‹‹` for Claude, and `›` before an answer.
//! - A call's line takes a `CallMark`, which gives no color, so a call that failed, which
//!   Claude deals with, stays grey after its `✗`.
//! - A notice takes a `NoticeMark`, which only a `Notice` writes, and which gives the notice its
//!   color, so red and orange only ever follow `!`, for what needs the user. A color never tells
//!   two lines apart by itself, since the marks do too.
//! - `Notice` takes its header from claudio and anything else as quoted text, which it keeps as
//!   written, unmarked, under the header, so claudio's words and others' never mix on a line,
//!   and the next mark ends a block.
//! - `part`, `parts`, `Hint` and `GROUP` join the pieces of a line.
//! - Every line here is as wide as its text, whatever the terminal's width, since the scrollback
//!   keeps a line long after the width it was written at changed.

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

/// The mark in front of a call's line, which says how far the call got. It leaves the line's
/// colors to the line, which keeps them out of the way, since a call is Claude's work and needs
/// nothing of the user, even when it fails.
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
/// its color, the only color a mark gives.
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

/// A notice: a header after its mark, in claudio's words, which follow this grammar, and the
/// text it quotes, like a hook's error output or what Claude Code says, under it as written, so
/// it copies as it was and is never mistaken for claudio's. Right under its header, the quoted
/// text reads as one block with it, and the next mark ends the block. A notice is the only way
/// to write a line with a notice's mark.
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
        let header: Line = (self.header.characters())
            .map(|(character, own)| (character, own.on(style)))
            .collect();
        let marked = Line::from([Span::new(self.mark.symbol(), style), " ".into()]);
        let quoted = self.quoted.iter().flat_map(|text| text.lines());
        std::iter::once(marked.with_line(header))
            .chain(quoted.map(|line| Notice::more(self.mark, line)))
            .collect()
    }

    /// A line more of the text a notice of `mark` quotes, for one whose text arrives a line at a
    /// time, like Claude Code's error output, right under the notice's last line.
    pub fn more(mark: NoticeMark, line: &str) -> Line {
        Line::from(Span::new(line, mark.style()))
    }
}

/// A line of what the user did, like a command, on the bar, where the user's lines
/// are, in the style of the label row's state. Only the user's lines are on the bar.
pub fn user_line(what: &str) -> Row {
    Line::from([marks(), Span::new(format!(" {what}"), SUBDUED_ON_BAR)]).on(YOURS)
}

/// The marks in front of a message, on a row of their own.
pub fn marks() -> Span {
    Span::new("››", MARK)
}

/// The marks in front of what Claude writes, on a row of their own, in the color of the user's
/// marks, since both say who starts.
pub fn reply_mark() -> Line {
    Line::from(Span::new("‹‹", MARK))
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

/// What a key does, which the user can learn from where they look, like `Ctrl+K stops`.
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

/// `parts` as one line's text, each set apart from the next.
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

/// What happened to a session, which a boundary names.
#[derive(Clone, Copy)]
pub enum SessionEnd {
    /// The context was cleared, which starts a new session.
    Cleared,
    /// claudio ended.
    Ended,
}

/// The lines that end a session: how to resume it, when its id is known, then the boundary
/// that names how it ended, so the hint stands on the side of the session it resumes.
pub fn session_end(end: SessionEnd, session: Option<&str>) -> Vec<Line> {
    let resume = session.map(|session| {
        let command = Span::new(format!("claudio --resume {session}"), CODE);
        Notice::new(
            NoticeMark::Noted,
            Line::from(["resume with ".into(), command]),
        )
        .lines()
    });
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
    resume.into_iter().flatten().chain([boundary]).collect()
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
        let stop = Hint::new("Ctrl+K", "stops");
        let edit = Hint::new("Enter", "edits");
        assert_eq!(
            with_hint("clear waits", &stop),
            "clear waits · Ctrl+K stops"
        );
        assert_eq!(hints(&[stop, edit]), "Ctrl+K stops · Enter edits");
    }

    #[test]
    fn a_session_ends_with_how_to_resume_it_then_its_boundary() {
        let lines = session_end(SessionEnd::Cleared, Some("a1"));
        let text: Vec<String> = lines.iter().map(Line::text).collect();
        assert_eq!(
            text,
            ["- resume with claudio --resume a1", "─── cleared ───"]
        );
        let unknown = session_end(SessionEnd::Ended, None);
        assert_eq!(
            unknown.iter().map(Line::text).collect::<Vec<_>>(),
            ["─── ended ───"]
        );
    }
}
