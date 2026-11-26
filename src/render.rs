//! The conversation as the terminal shows it: plain lines, appended and never redrawn, so the
//! terminal wraps them to its width and copies them whole.
//!
//! The input line stays open below the conversation, so the user can type at any time, and the
//! conversation reaches the terminal above it as text, each line followed by a line break once it
//! ended. Claude's prose reaches it as it streams in, as far as its colors are settled, and the
//! rest of the conversation a whole line at a time.

use std::collections::HashSet;

use std::fs::File;
use std::io::Write;
use std::num::NonZeroUsize;

use serde_json::{Map, Value};

use crate::cut::cut;
use crate::markdown::{Highlighter, code_span};
use crate::protocol::{
    Ask, AskKind, Level, Question, SettingsError, State, TaskStatus, ToolResult, ToolUse,
};
use crate::status::grouped;
use crate::work::{Call, End, Resulted, Work};

use crate::style::{DONE_OR_ADDED, FAILED_OR_REMOVED, STRONG, SUBDUED, Style, YOURS};
use crate::text::{Line, Row, Span, Text, Wrap};
use crate::wording::{
    CallMark, Hint, Notice, NoticeMark, SessionEnd, answer_label, marks, part, parts, reply_mark,
    session_end, summarized_reply_mark, user_line, with_hint,
};

/// The tools whose input carries a `description` of the call in Claude's words, which sums it up
/// in place of its input, still whole in the full view.
/// Seen with Claude Code 2.1.322: the field is optional, yet every one of 1,798 Bash calls over
/// 60 days of one user's sessions had it, so a Bash call shows its command only in the full view.
const DESCRIBED_TOOLS: [&str; 2] = ["Agent", "Bash"];

/// The type of the subagent that Claude Code starts by default, which Claude also often names.
/// Ref: <https://code.claude.com/docs/en/sub-agents>, on the built-in subagents
const DEFAULT_AGENT: &str = "general-purpose";

/// The field that sums up a call of each tool that has one, when the call has no description.
const SUMMARY_FIELDS: [(&str, &str); 10] = [
    ("Bash", "command"),
    ("Edit", "file_path"),
    ("ExitPlanMode", "planFilePath"),
    ("Glob", "pattern"),
    ("Grep", "pattern"),
    ("Read", "file_path"),
    // Whom the message goes to, which a subagent's id names by its tag instead. A name stays as
    // written, since Claude Code renames a subagent whose name another one that runs holds, and
    // a message can go to Claude, a teammate or another session too
    ("SendMessage", "to"),
    ("WebFetch", "url"),
    ("WebSearch", "query"),
    ("Write", "file_path"),
];

/// The conversation's text that waits to be printed above the input line, and what Claude does
/// meanwhile.
pub struct Output {
    /// Each line followed by a line break once it ended. Only Claude's text leaves its last line
    /// open, and any other line ends it first.
    text: Text,
    /// The file of the full view: the lines the screen shows, each followed by what it leaves
    /// out, like a tool's whole input under its call. It holds the conversation in place of the
    /// client, which writes each line to it as it adds the line to the screen's. It holds only
    /// what this client showed, so a resumed session starts it empty.
    full_view: File,
    /// How many lines the full view holds.
    full_view_lines: usize,
    position: Position,
    /// What sets the next line apart from the last one. It is written with that line, so
    /// nothing the user sends in between can land between the two.
    pending_break: Option<Break>,
    /// Whether the last of Claude's text blocks ended, so that a text block right after it
    /// starts with the mark of its own.
    text_ended: bool,
    /// Whether pieces of the text block that runs arrived, which its end then repeats.
    has_streamed: bool,
    /// Whether the text that shows is a narration, whose mark then says it is a summary.
    is_narration: bool,
    /// The kinds of what Claude Code sends that claudio knows nothing of, and said so once.
    unknown_kinds: HashSet<(&'static str, String)>,
    status: Status,
    /// The tool uses whose result repeats what the screen already shows.
    hidden_results: HashSet<String>,
    /// The columns a noisy line takes at most on screen.
    max_noisy_line_columns: NonZeroUsize,
    /// The columns a table of Claude's takes at most as a grid.
    max_table_columns: NonZeroUsize,
    /// How the user stopped the turn that runs, if they did.
    interruption: Option<Interruption>,
    highlighter: Highlighter,
    /// The lines of Claude Code's error output that wait for Claude to finish the text it
    /// writes.
    waiting_stderr: Vec<String>,
    /// The lines of the full view after the last line of Claude Code's error output, while it
    /// ends a block that the next line of it goes on.
    stderr_end: Option<usize>,
}

/// What Claude does, which the marker in front of the input line shows.
#[derive(Clone, PartialEq)]
pub enum Status {
    /// Claude waits for the user's next message.
    Idle,
    Working,
    Writing,
    /// Claude runs the tool named `tool`, in the tool use with the id `call`.
    Running {
        tool: String,
        call: String,
    },
    /// A slow permission check of the tool use, as for `Running`, runs.
    Checking {
        tool: String,
        call: String,
    },
}

/// How the user stopped a turn.
#[derive(Clone, Copy)]
pub enum Interruption {
    /// With Ctrl+C, or by refusing a tool, which the turn's end then says.
    Stop,
    /// With a message, which says it already. The turn may instead move its work to the
    /// background and end without being stopped.
    Message,
}

/// Where the conversation stands, which decides what sets the next line apart.
#[derive(Clone, Copy, PartialEq)]
enum Position {
    /// Before the first line, which nothing comes before.
    Start,
    /// In a section, which the next line continues when it belongs to the same section.
    In(Section),
    /// After the end of a turn, which nothing continues.
    TurnEnd,
}

/// What sets a section apart from the one before.
#[derive(Clone, Copy)]
enum Break {
    /// The mark that starts Claude's reply to the user's messages.
    Reply,
    /// A blank line, then the mark, which starts Claude's text after other lines of its reply,
    /// or a text block after another.
    Prose,
    Blank,
}

/// A part of the conversation, set apart from the next part.
#[derive(Clone, Copy, PartialEq)]
enum Section {
    Message,
    Text,
    Tools,
    Notice,
}

impl Output {
    pub fn new(
        full_view: File,
        max_noisy_line_columns: NonZeroUsize,
        max_table_columns: NonZeroUsize,
        prose_wrap: Wrap,
    ) -> Self {
        Output {
            text: Text::default(),
            full_view,
            full_view_lines: 0,
            position: Position::Start,
            pending_break: None,
            text_ended: false,
            has_streamed: false,
            is_narration: false,
            unknown_kinds: HashSet::new(),
            status: Status::Idle,
            hidden_results: HashSet::new(),
            max_noisy_line_columns,
            max_table_columns,
            interruption: None,
            highlighter: Highlighter::new(max_table_columns.get(), prose_wrap),
            waiting_stderr: Vec::new(),
            stderr_end: None,
        }
    }

    pub fn status(&self) -> Status {
        self.status.clone()
    }

    /// Shows how the task `task_id` ended in the background, under the tag of the call that
    /// started it, once the call's result came, which otherwise shows the end.
    pub fn show_task_end(
        &mut self,
        work: &mut Work,
        task_id: &str,
        status: TaskStatus,
        summary: &str,
    ) {
        let end = End {
            status,
            summary: summary.to_string(),
        };
        if let Some((call, end)) = work.end_task(task_id, end) {
            self.push_task_end(work, &call, &end);
        }
    }

    /// Adds the line of how the task of `call` ended, with Claude Code's summary for Claude in
    /// the full view, and only there for a subagent's own call, as its other lines. The line of
    /// a subagent's end counts its calls, which show only in the full view.
    fn push_task_end(&mut self, work: &Work, call: &Call, end: &End) {
        let (mark, how) = match end.status {
            TaskStatus::Completed => (CallMark::Succeeded, "completed"),
            TaskStatus::Failed => (CallMark::Failed, "failed"),
            TaskStatus::Stopped => (CallMark::Failed, "stopped"),
        };
        let subagent = (call.task.as_ref()).and_then(|task| work.subagent(task));
        let calls = subagent.map(|subagent| match subagent.calls {
            1 => "1 call".to_string(),
            calls => format!("{calls} calls"),
        });
        let mut what = vec![part(&call.subject(), how)];
        what.extend(calls);
        let what = parts(&what);
        let line = mark.line(Span::new(what, SUBDUED));
        if call.tag.is_claude() {
            self.begin(Section::Tools);
            self.push_detail(line);
        } else {
            self.write_full_view_line(&line);
        }
        for line in end.summary.lines() {
            self.write_full_view_line(&Line::from(format!("  {line}")));
        }
    }

    /// Takes the text that waits to be printed above the input line.
    pub fn take_text(&mut self) -> Text {
        std::mem::take(&mut self.text)
    }

    #[cfg(test)]
    fn take_rows(&mut self) -> Vec<Row> {
        self.take_text().into_rows()
    }

    /// The lines of the rows that wait, without the style of each row past its end.
    #[cfg(test)]
    fn take_lines(&mut self) -> Vec<Line> {
        self.take_rows().into_iter().map(Row::into_line).collect()
    }

    /// Shows the message the user sends, after all that Claude wrote before it, so the
    /// conversation reads in the order Claude sees it.
    pub fn show_message(&mut self, text: &str) {
        // A message follows the line before it, and starts what the reply mark then introduces
        self.pending_break = None;
        // Each message starts with marks of its own, as Claude Code keeps each message apart, also
        // those it delivers together in one turn
        self.emit(Line::from(marks()).on(YOURS));
        for line in text.split('\n') {
            self.emit(Line::from(line).on(YOURS));
        }
        self.position = Position::In(Section::Message);
        self.set_status(Status::Working);
        // An interrupt sent as its turn ended had no turn left to stop
        self.interruption = None;
    }

    /// Ends the session, as `end` ended it, with the id that resumes it, when it has one to
    /// resume, so the conversation reads as a new one from there.
    pub fn show_session_end(&mut self, end: SessionEnd, session: Option<&str>) {
        self.end_text();
        self.begin(Section::Notice);
        self.push_detail(session_end(end, session));
    }

    /// Marks a command the user runs, which Claude Code runs in place of Claude, so the
    /// conversation reads what comes of it as the command's.
    pub fn show_command(&mut self, command: &str) {
        self.show_user_line(&format!("/{command}"));
    }

    /// Adds a line of what the user did, in the user's section, as a message, so what answers it
    /// starts with the reply mark.
    fn show_user_line(&mut self, what: &str) {
        self.end_text();
        self.pending_break = None;
        self.emit(user_line(what));
        self.position = Position::In(Section::Message);
    }

    /// Notes that Claude Code compacted the context, from `before` tokens to `after`, when known.
    pub fn show_compacted(&mut self, before: u64, after: Option<u64>) {
        let after = after.map_or_else(String::new, |after| format!(" to {}", grouped(after)));
        let compacted = format!("compacted from {}{after} tokens", grouped(before));
        self.show_notice(Notice::new(NoticeMark::Noted, compacted));
    }

    /// Says once for each kind that claudio skips what Claude Code sent, `name` of `part`, which
    /// claudio knows nothing of, so the user learns that Claude Code may show more than claudio.
    /// The full view holds the `line` that carried it.
    pub fn show_unknown(&mut self, part: &'static str, name: &str, line: &str) {
        if !self.unknown_kinds.insert((part, name.to_string())) {
            return;
        }
        let skipped = format!("Claude Code's {name} {part}s are not shown");
        let show = Hint::new("Ctrl+O", "shows the first");
        self.show_notice(Notice::new(NoticeMark::Warning, with_hint(&skipped, &show)));
        self.write_full_view_line(&Line::from(format!("  {line}")));
    }

    /// Shows the user's answer to what a tool asks, after the question.
    pub fn show_answer(&mut self, question: &str, text: &str) {
        self.push(answer_label(question).with(text).on(YOURS));
    }

    /// Shows the user's decision on a tool use that Claude Code asked about, like `allowed #01
    /// Bash: Run the tests`, which names the call by its tag, and by its summary, since the call's
    /// line can be far above. The user's `reason`
    /// follows, when they gave one.
    pub fn show_decision(&mut self, work: &Work, ask: &Ask, decision: &str, reason: Option<&str>) {
        let tag =
            (work.call(&ask.tool_use_id)).map_or_else(String::new, |call| format!("{} ", call.tag));
        let summary = summarize(&ask.tool_name, &ask.input);
        let first = summary.text.lines().next().unwrap_or_default();
        let line = format!(
            "{decision} {}",
            part(&format!("{tag}{}", ask.tool_name), first)
        );
        self.push_noisy(&Line::from(line), Some(YOURS));
        if let Some(reason) = reason {
            self.show_answer("reason", reason);
        }
    }

    /// Notes that the user stopped the turn, whose end then says so in the user's terms.
    pub fn note_interrupt(&mut self, interruption: Interruption) {
        self.interruption = Some(interruption);
    }

    /// Hides the result of a tool the user refused, which repeats the refusal.
    pub fn hide_result(&mut self, tool_use_id: &str) {
        self.hidden_results.insert(tool_use_id.to_string());
    }

    pub fn note_state(&mut self, state: State) {
        match state {
            State::Idle => self.set_status(Status::Idle),
            // What Claude does in its turn says more, once it is known
            State::Running if self.status == Status::Idle => self.set_status(Status::Working),
            // The input line asks the user, which says more than any label
            State::Running | State::RequiresAction => {}
        }
    }

    /// Notes that a slow permission check of the tool use with `tool_use_id` started, when
    /// `is_checking`, or ended. The label row shows the check only while it shows that tool use,
    /// since Claude can run several tools at once.
    pub fn note_permission_check(&mut self, tool_use_id: &str, is_checking: bool) {
        let status = match (&self.status, is_checking) {
            (Status::Running { tool, call }, true) if call == tool_use_id => Status::Checking {
                tool: tool.clone(),
                call: call.clone(),
            },
            (Status::Checking { tool, call }, false) if call == tool_use_id => Status::Running {
                tool: tool.clone(),
                call: call.clone(),
            },
            _ => return,
        };
        self.set_status(status);
    }

    pub fn show_text(&mut self, text: &str) {
        self.has_streamed = true;
        self.begin(Section::Text);
        self.set_status(Status::Writing);
        // The break goes before the text, even before the highlighter settles any of it
        self.write_break();
        let colored = self.highlighter.push(text);
        self.write(colored);
    }

    /// Ends a block of Claude's text, whose whole `text` shows here when none of it streamed,
    /// as for a command's output, which Claude Code writes itself. The output puts its own empty
    /// row between blocks, so it drops the whitespace at the end of `text`.
    pub fn show_text_end(&mut self, text: &str) {
        let text = text.trim_end();
        if !std::mem::take(&mut self.has_streamed) && !text.is_empty() {
            self.show_text(text);
            self.has_streamed = false;
        }
        self.end_text();
        self.text_ended = true;
        self.set_status(Status::Working);
    }

    /// Shows a narration, which comes whole, and only as the API's summary of what Claude wrote.
    /// Being a block of its own, it starts with a mark, which says so.
    pub fn show_narration(&mut self, text: &str) {
        self.is_narration = true;
        self.show_text_end(text);
        self.is_narration = false;
    }

    /// Ends the conversation, since Claude Code ended. The text Claude was writing ends where it
    /// stopped, the error output that waited for it follows, and then how Claude Code exited,
    /// when it says more than that it exited with status 0. The line states it and leaves to
    /// the user what it means, since a status that is not 0 can come from the user too, like
    /// from quitting while Claude works.
    pub fn show_end(&mut self, exit: Option<&str>) {
        self.end_text();
        self.set_status(Status::Idle);
        if let Some(exit) = exit {
            self.show_notice(Notice::new(NoticeMark::Noted, exit));
        }
    }

    /// Adds the rest of the text Claude wrote, which the highlighter holds.
    fn end_text(&mut self) {
        let colored = self.highlighter.finish();
        self.write(colored);
    }

    /// Shows a call, on a line of the conversation for Claude's, and in the full view for a
    /// subagent's.
    ///
    /// Undocumented: [`crate::contract::a_message_to_a_subagent_resumes_it`], for a call that
    /// names a subagent by its id, in `to`
    pub fn show_tool_use(&mut self, work: &mut Work, tool_use: &ToolUse) {
        if !tool_use.is_subagent() {
            self.set_status(Status::Running {
                tool: tool_use.name.clone(),
                call: tool_use.id.clone(),
            });
        }
        // Its ask shows the questions, and the input line the answers
        if tool_use.name == "AskUserQuestion" {
            self.hidden_results.insert(tool_use.id.clone());
            return;
        }
        let tag = work.add_call(tool_use);
        // One line per call keeps the tool calls out of the way of the conversation. A call that
        // names a task, like TaskStop's, names it by the tag of the call that started it, and
        // one that names a subagent, like SendMessage's, which resumes it, by the tag of the
        // call that first started the subagent
        let target = (work.task_tag(read_field(&tool_use.input, "task_id"))).or_else(|| {
            let subagent = work.subagent(read_field(&tool_use.input, "to"))?;
            subagent.start.clone()
        });
        let mut line = CallMark::Started.line(Line::from([
            Span::new(tag.to_string(), SUBDUED),
            Span::new(format!(" {}:", tool_use.name), STRONG),
            " ".into(),
        ]));
        let summary = summarize(&tool_use.name, &tool_use.input);
        let is_input = target.is_none() && summary.is_input;
        if let Some(target) = target {
            line.push(Span::new(target.to_string(), SUBDUED));
        } else {
            let mut summary_lines = summary.text.lines();
            let first = summary_lines.next().unwrap_or_default();
            line.push(if is_input {
                code_span(first)
            } else {
                Span::from(first)
            });
            match summary_lines.count() {
                0 => {}
                count => line.append(Line::from([
                    " ".into(),
                    Span::new(format!("(+{count} lines)"), SUBDUED),
                ])),
            }
        }
        if tool_use.name == "Edit" {
            let removed = read_field(&tool_use.input, "old_string").lines().count();
            let added = read_field(&tool_use.input, "new_string").lines().count();
            line.append(Line::from([
                " ".into(),
                Span::new(format!("-{removed}"), FAILED_OR_REMOVED),
                " ".into(),
                Span::new(format!("+{added}"), DONE_OR_ADDED),
            ]));
        }
        // A subagent's calls show only in the full view, since what it did shows in what it
        // reports back to Claude, and its end line counts them
        if tool_use.is_subagent() {
            self.write_full_view_line(&line);
        } else if is_input {
            self.begin(Section::Tools);
            self.push_noisy(&line, None);
        } else {
            self.begin(Section::Tools);
            self.push_detail(line);
        }
        // The call's line ends the full view now
        work.set_line(&tool_use.id, self.full_view_lines);
        for (field, value) in &tool_use.input {
            let name = Line::from(["  ".into(), Span::new(format!("{field}:"), SUBDUED)]);
            if let Value::String(text) = value {
                self.write_full_view_line(&name);
                for line in text.lines() {
                    self.write_full_view_line(&Line::from(format!("    {line}")));
                }
            } else {
                self.write_full_view_line(&name.with(format!(" {value}")));
            }
        }
    }

    pub fn show_tool_result(&mut self, work: &mut Work, result: &ToolResult) {
        if !result.is_subagent() {
            self.set_status(Status::Working);
        }
        let resulted = work.take_result(&result.tool_use_id);
        let text = result.text.as_deref().map_or("(no text)", str::trim);
        // The outcome is all the line adds to the call's line, so the rest stays in the back
        let mark = if result.is_error {
            CallMark::Failed
        } else {
            CallMark::Succeeded
        };
        let first = text.lines().next().unwrap_or("(no output)");
        let more = match text.lines().count().saturating_sub(1) {
            0 => String::new(),
            count => format!(" (+{count} lines)"),
        };
        let call = match &resulted {
            Resulted::Done(call) => call.as_ref(),
            Resulted::Background(call, _) | Resulted::Subagent(call, _) => Some(call),
        };
        // Other lines can come between a call and its result, so the result stands on its own
        let shown = format!("{first}{more}");
        let shown = match call {
            Some(call) => part(&call.subject(), &shown),
            None => shown,
        };
        let line = mark.line(Span::new(shown, SUBDUED));
        // Like its call, a subagent's result shows only in the full view
        if result.is_subagent() {
            self.write_full_view_line(&line);
            for line in text.lines().skip(1) {
                self.write_full_view_line(&Line::from(format!("  {line}")));
            }
            return;
        }
        // A tool the interrupt stopped fails with Claude Code's message to Claude
        if self.hidden_results.remove(&result.tool_use_id)
            || (self.interruption.is_some() && result.is_error)
        {
            return;
        }
        self.begin(Section::Tools);
        match resulted {
            // A call whose task goes on in the background has only started, and its result is
            // Claude Code's note to Claude with the task's id and output file, so the line says
            // that the task runs, and the task's end shows the outcome
            Resulted::Background(call, end) => {
                let running = part(&call.subject(), "running in background");
                self.push_detail(CallMark::Running.line(Span::new(running, SUBDUED)));
                for line in text.lines() {
                    self.write_full_view_line(&Line::from(format!("  {line}")));
                }
                if let Some(end) = end {
                    self.push_task_end(work, &call, &end);
                }
            }
            // The result of a call that started a subagent in the foreground is the subagent's
            // report, which the full view holds, under the line of how the subagent ended, which
            // its task told before the result, or else the result tells
            Resulted::Subagent(call, end) => {
                let failed = if result.is_error {
                    TaskStatus::Failed
                } else {
                    TaskStatus::Completed
                };
                let end = End {
                    status: end.map_or(failed, |end| end.status),
                    summary: text.to_string(),
                };
                self.push_task_end(work, &call, &end);
            }
            Resulted::Done(_) => {
                self.push_noisy(&line, None);
                for line in text.lines().skip(1) {
                    self.write_full_view_line(&Line::from(format!("  {line}")));
                }
            }
        }
    }

    /// Shows what `ask` asks, and gives the line of the full view, from 1, that each of its
    /// questions starts on.
    pub fn show_ask(&mut self, work: &Work, ask: &Ask) -> Vec<usize> {
        self.begin(Section::Tools);
        match &ask.kind {
            // The call's line shows what it asks to run, and the full view shows its input whole
            // under it, so the request opens there, whatever came after the call, like the other
            // calls of the same reply. The reason for asking adds to the full view's end, right
            // under the call when nothing came between. Claude Code shows a call before it asks
            // to allow it, so only a call it never showed opens at the end
            AskKind::Permission(permission) => {
                let line = (work.call(&ask.tool_use_id))
                    .map_or(self.full_view_lines.max(1), |call| call.line);
                if let Some(reason) = &permission.reason {
                    let name = Line::from(["  ".into(), Span::new("reason:", SUBDUED)]);
                    self.write_full_view_line(&name);
                    for reason_line in reason.lines() {
                        self.write_full_view_line(&Line::from(format!("    {reason_line}")));
                    }
                }
                vec![line]
            }
            // The plan shows whole, since the answer depends on all of it
            AskKind::Plan(plan) => {
                vec![self.push_details(markdown_lines(plan, self.max_table_columns.get()))]
            }
            AskKind::Questions(questions) => questions
                .iter()
                .map(|question| self.push_details(question_lines(question)))
                .collect(),
        }
    }

    /// Shows how a turn ended, when it ended before its work was done. Claude Code ends a turn
    /// the user stopped as one that failed, like any other, so the line says it in the user's
    /// terms.
    pub fn show_turn_end(&mut self, failure: Option<&str>) {
        // A message makes the turn either stop or move its work to the background and end well
        let line = match (failure, self.interruption.take()) {
            (None, _) | (Some(_), Some(Interruption::Message)) => None,
            (Some(_), Some(Interruption::Stop)) => Some("stopped".to_string()),
            (Some(reason), None) => Some(part("turn ended", reason)),
        };
        if let Some(line) = line {
            self.show_notice(Notice::new(NoticeMark::Noted, line));
        }
        // A message that waits for its reply still waits when an older turn ends
        if self.position != Position::In(Section::Message) {
            self.position = Position::TurnEnd;
        }
    }

    pub fn show_settings_errors(&mut self, errors: &[SettingsError]) {
        for error in errors {
            let file = error.file.as_deref().unwrap_or("a settings file");
            let skipped = format!("Claude Code skipped {file}, so none of its settings apply");
            let why = format!("{} {}", error.path, error.message);
            self.show_notice(Notice::new(NoticeMark::Error, skipped).quoting(&why));
        }
    }

    /// Warns that claudio runs a Claude Code other than the one it was checked against, whose
    /// changes may break what claudio assumes, like the shape of what it writes.
    pub fn show_unchecked_version(&mut self, version: &str) {
        let warning = format!(
            "Claude Code {version} runs, but {} is the one checked to work",
            crate::claude_code::VERSION
        );
        self.show_notice(Notice::new(NoticeMark::Warning, warning));
    }

    /// Warns that a resize can leave copies of the live rows in the scrollback, since frames can
    /// only count their way back to the first of them in this terminal.
    pub fn show_inexact_redraw(&mut self) {
        self.show_notice(Notice::new(
            NoticeMark::Warning,
            "a resize can leave copies of the label row and box in the scrollback, which tmux 3.5 avoids",
        ));
    }

    pub fn show_no_wrap_at_spaces(&mut self) {
        self.show_notice(Notice::new(
            NoticeMark::Warning,
            "the terminal cannot wrap Claude's prose at spaces, so it wraps at the edge",
        ));
    }

    pub fn show_paying_key(&mut self, key: &str) {
        let warning = format!("{key} pays for this session, not the subscription");
        self.show_notice(Notice::new(NoticeMark::Warning, warning));
    }

    /// Shows that the hook `hook` failed with `exit_code`, or was cancelled, when
    /// `is_cancelled`, then its error output, under it.
    pub fn show_hook_failure(
        &mut self,
        hook: &str,
        exit_code: Option<i64>,
        stderr: &str,
        is_cancelled: bool,
    ) {
        let how = match exit_code {
            _ if is_cancelled => "was cancelled".to_string(),
            Some(code) => format!("failed with exit code {code}"),
            None => "failed".to_string(),
        };
        let failed = Notice::new(NoticeMark::Error, format!("{hook} hook {how}"));
        self.show_notice(failed.quoting(stderr));
    }

    /// Shows what Claude Code tells the user at `level`, under a header that says it is Claude
    /// Code's: in the full view only when it is `Info`, as a warning when it is a `Warning`, like
    /// `Args from unknown skill: <name>`, and else noted, since a suggestion, like `Session
    /// resumed`, needs nothing of the user. `SDKInformationalMessage` asks to show a suggestion
    /// "prominently, as a warning is", but a warning that needs nothing teaches the user to skip
    /// warnings.
    /// Ref: `SDKInformationalMessage` in `sdk.d.ts`
    /// Ref: <https://developer.dynatrace.com/design/status-and-health>, which warns that
    /// "over-communicating statuses can overwhelm users"
    pub fn show_claude_code_notice(&mut self, text: &str, level: Level) {
        let notice = match level {
            Level::Info => {
                for line in text.lines() {
                    self.write_full_view_line(&Line::from(line));
                }
                return;
            }
            Level::Notice | Level::Suggestion => {
                Notice::new(NoticeMark::Noted, "Claude Code notes")
            }
            Level::Warning => Notice::new(NoticeMark::Warning, "Claude Code warns"),
        };
        self.show_notice(notice.quoting(text));
    }

    /// Shows an error of claudio's, which it words itself.
    pub fn show_error(&mut self, error: &str) {
        self.show_notice(Notice::new(NoticeMark::Error, error));
    }

    /// Shows the error of a request to the API, which Claude Code words.
    pub fn show_api_error(&mut self, text: &str) {
        self.show_notice(Notice::new(NoticeMark::Error, "the API request failed").quoting(text));
    }

    /// Shows `notice` in a section of its own.
    fn show_notice(&mut self, notice: Notice) {
        self.begin(Section::Notice);
        for line in notice.lines() {
            self.push_detail(line);
        }
    }

    /// Shows a line of Claude Code's error output. Claude's text has nothing that names it, so
    /// the line waits until Claude finishes writing and the paragraphs stay whole. Every other
    /// line says what it is, like a tool's result, which names its tool, so the line goes in at
    /// once, between a tool's call and its result too.
    pub fn show_stderr(&mut self, line: &str) {
        if self.status == Status::Writing {
            self.waiting_stderr.push(line.to_string());
        } else {
            self.push_stderr(line);
        }
    }

    pub fn has_waiting_stderr(&self) -> bool {
        !self.waiting_stderr.is_empty()
    }

    /// Seen with Claude Code 2.1.322: it writes to its error output only on failure, like the
    /// tag of an API error, so each line is red, as a failure.
    fn push_stderr(&mut self, line: &str) {
        // A line right after the last one, with nothing written in between, goes on its block
        if self.stderr_end == Some(self.full_view_lines) {
            self.push_detail(Notice::more(NoticeMark::Error, line));
        } else {
            let output = Notice::new(NoticeMark::Error, "Claude Code's error output").quoting(line);
            self.show_notice(output);
        }
        self.stderr_end = Some(self.full_view_lines);
    }

    /// Sets what Claude does. Once Claude stops writing, the lines of error output that waited
    /// follow its text.
    fn set_status(&mut self, status: Status) {
        if self.status == Status::Writing && status != Status::Writing {
            for line in std::mem::take(&mut self.waiting_stderr) {
                self.push_stderr(&line);
            }
        }
        self.status = status;
    }

    /// Starts a section, set apart by the next line written: Claude's reply from the user's
    /// messages with a mark, which tells them apart in copied text too, where colors are gone,
    /// Claude's text with the mark too, so it stands out among the lines of tool calls and
    /// notices, and another section, or what follows the end of a turn, with a blank line.
    fn begin(&mut self, section: Section) {
        let text_ended = std::mem::take(&mut self.text_ended);
        let pending_break = match (self.pending_break, self.position) {
            // A reply that printed nothing yet still starts with its mark
            (Some(Break::Reply), _) | (_, Position::In(Section::Message)) => Some(Break::Reply),
            (_, Position::Start) if section == Section::Text => Some(Break::Reply),
            (_, Position::TurnEnd | Position::In(Section::Tools | Section::Notice))
                if section == Section::Text =>
            {
                Some(Break::Prose)
            }
            // A text block after another starts with the mark of its own, as Claude Code gives
            // each its own dot
            (_, Position::In(Section::Text)) if section == Section::Text && text_ended => {
                Some(Break::Prose)
            }
            (_, Position::TurnEnd) => Some(Break::Blank),
            (_, Position::In(current)) if current != section => Some(Break::Blank),
            (pending_break, Position::Start | Position::In(_)) => pending_break,
        };
        self.pending_break = pending_break;
        self.position = Position::In(section);
    }

    /// Adds a whole row, after the break that sets it apart, if one waits.
    fn push(&mut self, row: impl Into<Row>) {
        self.write_break();
        self.emit(row);
    }

    fn write_break(&mut self) {
        match self.pending_break.take() {
            Some(Break::Reply) => self.emit(self.reply_mark()),
            Some(Break::Prose) => {
                self.emit(Line::default());
                self.emit(self.reply_mark());
            }
            Some(Break::Blank) => self.emit(Line::default()),
            None => {}
        }
    }

    fn reply_mark(&self) -> Line {
        if self.is_narration {
            summarized_reply_mark()
        } else {
            reply_mark()
        }
    }

    /// Adds a detail line, after the break that sets it apart, if one waits. A detail line is any
    /// line other than the conversation itself, the user's words and Claude's prose.
    fn push_detail(&mut self, line: Line) {
        self.write_break();
        self.emit(line);
    }

    /// Adds a noisy detail line: cut on screen, and whole in the full view. A noisy line shows a
    /// tool's input or output as it is, so it can run to hundreds of columns.
    ///
    /// The cut here is the only one a noisy line gets, so callers pass their text whole, and the
    /// full view shows it as it was. On screen, the line's row takes `background`, when given.
    fn push_noisy(&mut self, line: &Line, background: Option<Style>) {
        self.write_break();
        self.end_claude_line();
        self.write_full_view_line(line);
        let row = Row::from(cut(line, self.max_noisy_line_columns));
        self.text.push_line(match background {
            Some(background) => row.on(background),
            None => row,
        });
    }

    /// Adds a whole row to the screen and its line to the full view.
    fn emit(&mut self, row: impl Into<Row>) {
        let row = row.into();
        self.end_claude_line();
        self.write_full_view_line(row.line());
        self.text.push_line(row);
    }

    /// Ends Claude's line where Claude got to, as when a message interrupts Claude mid-line.
    fn end_claude_line(&mut self) {
        let colored = self.highlighter.end_line();
        self.write(colored);
    }

    /// Adds text to the screen and to the full view.
    fn write(&mut self, text: Text) {
        self.write_full_view_text(&text.to_pager());
        self.text.append(text);
    }

    /// Adds a line to the full view only.
    fn write_full_view_line(&mut self, line: &Line) {
        self.write_full_view_text(&format!("{line}\n"));
    }

    /// Adds text to the full view, as a pager shows it. A write that fails, like on a full disk,
    /// leaves the text out of the full view only.
    fn write_full_view_text(&mut self, text: &str) {
        self.full_view_lines += text.matches('\n').count();
        let _ = self.full_view.write_all(text.as_bytes());
    }

    /// Adds detail lines, and gives the line of the full view, from 1, that the first one is on.
    fn push_details(&mut self, lines: Vec<Line>) -> usize {
        self.write_break();
        self.end_claude_line();
        let first = self.full_view_lines + 1;
        for line in lines {
            self.push_detail(line);
        }
        first
    }
}

/// The lines of Markdown, highlighted, with its tables as grids up to `max_table_columns`.
pub fn markdown_lines(markdown: &str, max_table_columns: usize) -> Vec<Line> {
    let mut highlighter = Highlighter::new(max_table_columns, Wrap::Edge);
    let mut colored = highlighter.push(markdown.trim());
    colored.append(highlighter.finish());
    colored
        .into_rows()
        .into_iter()
        .map(Row::into_line)
        .collect()
}

/// The lines of a question: its header and text, then its options, numbered from 1 as the question
/// view numbers them.
pub fn question_lines(question: &Question) -> Vec<Line> {
    let mut lines = vec![Line::from([
        Span::new(question.header.as_str(), STRONG),
        format!(" {}", question.text).into(),
    ])];
    for (number, choice) in (1..).zip(&question.options) {
        lines.push(Line::from([
            format!("  {number}. {} ", choice.label).into(),
            Span::new(choice.description.as_str(), SUBDUED),
        ]));
        if let Some(preview) = &choice.preview {
            for line in preview.lines() {
                lines.push(Line::from(["     ".into(), Span::new(line, SUBDUED)]));
            }
        }
    }
    lines
}

/// What sums up a call on its line.
struct Summary {
    text: String,
    /// Whether the text is the call's input, as written, rather than Claude's words about it,
    /// which the line tells apart by showing the input as code.
    is_input: bool,
}

/// The call's description for a tool that has one, or else its input: the summary field for a
/// tool that has one, and all of it in JSON otherwise.
fn summarize(tool: &str, input: &Map<String, Value>) -> Summary {
    let text = |field: &str| input.get(field).and_then(Value::as_str);
    if DESCRIBED_TOOLS.contains(&tool)
        && let Some(description) = text("description")
    {
        // A subagent of a type other than the default one is named by it, since it runs with
        // a prompt and tools of its own
        let text = match text("subagent_type") {
            Some(kind) if tool == "Agent" && kind != DEFAULT_AGENT => {
                format!("{kind} · {description}")
            }
            _ => description.to_string(),
        };
        return Summary {
            text,
            is_input: false,
        };
    }
    let field = SUMMARY_FIELDS
        .iter()
        .find(|(name, _)| *name == tool)
        .and_then(|(_, field)| text(field));
    // The line's cut shortens it, once
    let text = field.map_or_else(
        || serde_json::to_string(input).expect("a JSON object turns back into JSON"),
        str::to_string,
    );
    Summary {
        text,
        is_input: true,
    }
}

fn read_field<'a>(input: &'a Map<String, Value>, field: &str) -> &'a str {
    input.get(field).and_then(Value::as_str).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::contract;
    use crate::protocol::Event;

    use super::*;
    use crate::config::Config;
    use crate::protocol::Task;
    use crate::style::{CODE, PLAIN};
    use crate::work::Place;

    /// The start of a call's line, before its summary, like `○ #00 Read: `.
    fn call_line(tag: &str, tool: &str) -> Line {
        Line::styled([
            ("○", STRONG),
            (" ", PLAIN),
            (tag, SUBDUED),
            (&format!(" {tool}:"), STRONG),
            (" ", PLAIN),
        ])
    }

    /// A line of `text` after `outcome`, like a result's or a task's end.
    fn outcome_line(outcome: &str, text: &str) -> Line {
        Line::styled([(&format!("{outcome} "), PLAIN), (text, SUBDUED)])
    }

    /// The output with the work it draws, side by side as the session holds them, which takes
    /// the events that draw the work.
    struct Shown {
        output: Output,
        work: Work,
    }

    impl Shown {
        fn new(full_view: File) -> Self {
            Shown {
                output: Output::new(
                    full_view,
                    Config::default().max_noisy_line_columns,
                    Config::default().max_table_columns,
                    Wrap::Edge,
                ),
                work: Work::default(),
            }
        }

        fn show_tool_use(&mut self, tool_use: &ToolUse) {
            self.output.show_tool_use(&mut self.work, tool_use);
        }

        fn show_tool_result(&mut self, result: &ToolResult) {
            self.output.show_tool_result(&mut self.work, result);
        }

        fn show_task_end(&mut self, task_id: &str, status: TaskStatus, summary: &str) {
            (self.output).show_task_end(&mut self.work, task_id, status, summary);
        }

        fn show_ask(&mut self, ask: &Ask) -> Vec<usize> {
            self.output.show_ask(&self.work, ask)
        }
    }

    impl std::ops::Deref for Shown {
        type Target = Output;

        fn deref(&self) -> &Output {
            &self.output
        }
    }

    impl std::ops::DerefMut for Shown {
        fn deref_mut(&mut self) -> &mut Output {
            &mut self.output
        }
    }

    fn new_output() -> Shown {
        Shown::new(tempfile::tempfile().expect("the temporary directory takes a file"))
    }

    fn output_after_message() -> Shown {
        let mut output = new_output();
        output.show_message("Hi");
        output.take_lines();
        output
    }

    #[test]
    fn a_reply_starts_with_its_mark() {
        let mut output = output_after_message();
        output.show_text("Hello.\n");
        assert_eq!(output.take_lines(), [reply_mark(), Line::from("Hello.")]);
    }

    #[test]
    fn error_output_waits_for_the_text_claude_writes() {
        let mut output = output_after_message();
        output.show_text("One.\n\nTw");
        output.show_stderr("Failed");
        output.show_text("o.\n");
        output.show_text_end("");
        let lines = output.take_lines();
        assert_eq!(
            lines[..4],
            [
                reply_mark(),
                Line::from("One."),
                Line::default(),
                Line::from("Two.")
            ]
        );
        let output = Notice::new(NoticeMark::Error, "Claude Code's error output").quoting("Failed");
        assert_eq!(lines[5..], output.lines());
    }

    #[test]
    fn lines_of_error_output_in_a_row_make_one_block() {
        let mut output = output_after_message();
        output.show_stderr("one");
        output.show_stderr("two");
        output.show_compacted(2, None);
        output.show_stderr("three");
        let text: Vec<String> = output.take_lines().iter().map(Line::text).collect();
        assert_eq!(
            text[1..],
            [
                "! Claude Code's error output",
                "one",
                "two",
                "- compacted from 2 tokens",
                "! Claude Code's error output",
                "three"
            ]
        );
    }

    #[test]
    fn a_message_sent_mid_line_follows_what_claude_wrote_of_the_line() {
        let mut output = output_after_message();
        output.show_text("Hel");
        output.show_message("Stop");
        let rows = output.take_rows();
        assert_eq!(rows[..2], [reply_mark().into(), Line::from("Hel").into()]);
        assert_eq!(rows[3], Line::from("Stop").on(YOURS));
    }

    #[test]
    fn a_line_from_elsewhere_ends_claudes_line_where_it_got_to() {
        let mut output = output_after_message();
        output.show_text("Hel");
        output.show_error("Failed");
        assert_eq!(
            output.take_lines(),
            [
                reply_mark(),
                Line::from("Hel"),
                Line::default(),
                Notice::new(NoticeMark::Error, "Failed").lines()[0].clone()
            ]
        );
    }

    #[test]
    fn a_line_start_the_highlighter_holds_follows_the_mark() {
        let mut output = output_after_message();
        output.show_text("##");
        output.show_message("Stop");
        let lines = output.take_lines();
        assert_eq!(lines[..2], [reply_mark(), Line::from("##")]);
    }

    #[test]
    fn a_reply_that_starts_with_a_tool_starts_with_its_mark() {
        let mut output = output_after_message();
        let mut input = Map::new();
        input.insert("command".to_string(), Value::String("ls".to_string()));
        output.show_tool_use(&ToolUse {
            id: "1".to_string(),
            name: "Bash".to_string(),
            input,
            subagent: None,
        });
        assert_eq!(output.take_lines()[0], reply_mark());
    }

    #[test]
    fn a_turn_a_message_stopped_ends_without_a_word() {
        let mut output = output_after_message();
        output.show_message("Stop");
        output.note_interrupt(Interruption::Message);
        output.take_lines();
        output.show_turn_end(Some("error_during_execution"));
        assert_eq!(output.take_lines(), Vec::<Line>::new());
    }

    #[test]
    fn a_turn_claude_starts_by_itself_is_set_apart_from_the_turn_before() {
        let mut output = output_after_message();
        output.show_text("Hi.\n");
        output.show_turn_end(None);
        output.show_text("Done.\n");
        assert_eq!(
            output.take_lines(),
            [
                reply_mark(),
                Line::from("Hi."),
                Line::default(),
                reply_mark(),
                Line::from("Done.")
            ]
        );
    }

    #[test]
    fn text_after_a_tool_starts_with_the_mark() {
        let mut output = output_after_message();
        output.show_tool_use(&read_call("1", "a.txt"));
        output.show_text("Read it.\n");
        let lines = output.take_lines();
        assert_eq!(
            lines[2..],
            [Line::default(), reply_mark(), Line::from("Read it.")]
        );
    }

    #[test]
    fn each_text_block_in_a_row_starts_with_the_mark() {
        let mut output = output_after_message();
        output.show_text("One.");
        output.show_text_end("");
        output.show_text("Two.");
        output.show_text_end("");
        assert_eq!(
            output.take_lines(),
            [
                reply_mark(),
                Line::from("One."),
                Line::default(),
                reply_mark(),
                Line::from("Two.")
            ]
        );
    }

    #[test]
    fn a_reply_keeps_its_mark_when_the_turn_its_message_stopped_ends_first() {
        let mut output = output_after_message();
        output.show_message("Stop");
        output.note_interrupt(Interruption::Message);
        output.take_lines();
        output.show_turn_end(Some("error_during_execution"));
        output.show_text("Stopped.\n");
        assert_eq!(output.take_lines(), [reply_mark(), Line::from("Stopped.")]);
    }

    /// Reads back what the output wrote to the full view, after the user's first message.
    fn full_view_after(show: impl FnOnce(&mut Shown)) -> Vec<String> {
        let file = tempfile::NamedTempFile::new().expect("the temporary directory takes a file");
        let mut output = Shown::new(file.reopen().expect("the file opens again"));
        output.show_message("Hi");
        show(&mut output);
        let text = std::fs::read_to_string(file.path()).expect("the full view reads back");
        text.lines().skip(2).map(str::to_string).collect()
    }

    #[test]
    fn each_question_starts_on_the_full_view_line_its_ask_gives() {
        let file = tempfile::NamedTempFile::new().expect("the temporary directory takes a file");
        let mut output = Shown::new(file.reopen().expect("the file opens again"));
        output.show_message("Hi");
        output.show_text("Two questions:");
        let request = json!({
            "subtype": "can_use_tool",
            "tool_name": "AskUserQuestion",
            "tool_use_id": "1",
            "input": {"questions": [
                {"question": "Which database?", "header": "Database",
                 "options": [{"label": "Postgres", "description": "a\nserver"}]},
                {"question": "Which checks?", "header": "Checks", "options": []},
            ]},
        });
        let Ok(crate::protocol::Event::Ask(ask)) =
            crate::protocol::read_request("request-1".to_string(), request)
        else {
            panic!("an ask");
        };
        let starts = output.show_ask(&ask);
        let text = std::fs::read_to_string(file.path()).expect("the full view reads back");
        let lines: Vec<&str> = text.lines().collect();
        let headers: Vec<&str> = starts.iter().map(|start| lines[start - 1]).collect();
        let header = |header, text| Line::styled([(header, STRONG), (text, PLAIN)]).to_string();
        assert_eq!(
            headers,
            [
                header("Database", " Which database?"),
                header("Checks", " Which checks?"),
            ]
        );
    }

    #[test]
    fn a_request_to_allow_a_call_opens_on_the_calls_line() {
        let file = tempfile::NamedTempFile::new().expect("the temporary directory takes a file");
        let mut output = Shown::new(file.reopen().expect("the file opens again"));
        output.show_message("Hi");
        for (id, command) in [("call-a", "touch a"), ("call-b", "touch b")] {
            output.show_tool_use(&ToolUse {
                id: id.to_string(),
                ..tool_use("Bash", json!({"command": command}))
            });
        }
        let request = json!({
            "subtype": "can_use_tool",
            "tool_name": "Bash",
            "tool_use_id": "call-a",
            "input": {"command": "touch a"},
        });
        let Ok(crate::protocol::Event::Ask(ask)) =
            crate::protocol::read_request("request-1".to_string(), request)
        else {
            panic!("an ask");
        };
        let starts = output.show_ask(&ask);
        let text = std::fs::read_to_string(file.path()).expect("the full view reads back");
        let lines: Vec<&str> = text.lines().collect();
        let line = lines[starts[0] - 1];
        assert!(line.contains("Bash:") && line.contains("touch a"), "{line}");
    }

    #[test]
    fn the_full_view_shows_each_field_of_a_call_under_it() {
        let lines = full_view_after(|output| {
            output.show_tool_use(&ToolUse {
                id: "1".to_string(),
                name: "Bash".to_string(),
                input: json!({"command": "cd a\nmake", "timeout": 5})
                    .as_object()
                    .expect("an object")
                    .clone(),
                subagent: None,
            });
        });
        assert_eq!(
            lines[2..],
            [
                Line::styled([("  ", PLAIN), ("command:", SUBDUED)]).to_string(),
                "    cd a".to_string(),
                "    make".to_string(),
                Line::styled([("  ", PLAIN), ("timeout:", SUBDUED), (" 5", PLAIN)]).to_string(),
            ]
        );
    }

    #[test]
    fn the_full_view_shows_the_rest_of_a_result_of_several_lines_under_it() {
        let lines = full_view_after(|output| {
            output.show_tool_use(&read_call("1", "a.txt"));
            output.show_tool_result(&result("1", "one\ntwo"));
            output.show_tool_use(&read_call("2", "b.txt"));
            output.show_tool_result(&result("2", "three"));
        });
        assert_eq!(
            lines[4..],
            [
                outcome_line("✓", "#00 Read: one (+1 lines)").to_string(),
                "  two".to_string(),
                call_line("#01", "Read")
                    .with(code_span("b.txt"))
                    .to_string(),
                Line::styled([("  ", PLAIN), ("file_path:", SUBDUED)]).to_string(),
                "    b.txt".to_string(),
                outcome_line("✓", "#01 Read: three").to_string(),
            ]
        );
    }

    #[test]
    fn a_noisy_line_is_cut_on_screen_and_whole_in_the_full_view() {
        let path = "a".repeat(100);
        let mut screen = Vec::new();
        let lines = full_view_after(|output| {
            output.take_lines();
            output.show_tool_use(&read_call("1", &path));
            screen = output.take_lines();
        });
        let call = call_line("#00", "Read").with(code_span(&path));
        assert_eq!(
            screen[1],
            cut(&call, Config::default().max_noisy_line_columns)
        );
        assert_ne!(screen[1], call);
        assert_eq!(lines[1], call.to_string());
    }

    #[test]
    fn a_call_with_a_description_stays_whole() {
        let description = "x".repeat(100);
        let mut input = Map::new();
        input.insert("command".to_string(), Value::String("ls".to_string()));
        input.insert(
            "description".to_string(),
            Value::String(description.clone()),
        );
        let mut output = output_after_message();
        output.show_tool_use(&ToolUse {
            id: "1".to_string(),
            name: "Bash".to_string(),
            input,
            subagent: None,
        });
        assert!(output.take_lines()[1].text().contains(&description));
    }

    #[test]
    fn an_error_and_an_answer_stay_whole() {
        let long = "x".repeat(100);
        let mut output = output_after_message();
        output.show_error(&long);
        output.show_answer("Q", &long);
        let lines = output.take_lines();
        assert_eq!(
            lines[1],
            Notice::new(NoticeMark::Error, long.clone()).lines()[0]
        );
        assert!(lines[2].text().contains(&long));
    }

    #[test]
    fn the_full_view_leaves_out_the_codes_that_fill_a_row() {
        let file = tempfile::NamedTempFile::new().expect("the temporary directory takes a file");
        let mut output = Shown::new(file.reopen().expect("the file opens again"));
        output.show_message("Hi");
        let text = std::fs::read_to_string(file.path()).expect("the full view reads back");
        assert!(!text.contains("\x1b[K"), "{text:?}");
    }

    fn read_call(id: &str, path: &str) -> ToolUse {
        let mut input = Map::new();
        input.insert("file_path".to_string(), Value::String(path.to_string()));
        ToolUse {
            id: id.to_string(),
            name: "Read".to_string(),
            input,
            subagent: None,
        }
    }

    fn result(id: &str, text: &str) -> ToolResult {
        ToolResult {
            tool_use_id: id.to_string(),
            text: Some(text.to_string()),
            is_error: false,
            subagent: None,
        }
    }

    #[test]
    fn a_result_repeats_its_call_tag_and_names_its_tool() {
        let mut output = output_after_message();
        output.show_tool_use(&read_call("1", "a.txt"));
        output.show_tool_use(&read_call("2", "b.txt"));
        output.show_tool_result(&result("2", "b"));
        let lines = output.take_lines();
        assert_eq!(lines[2], call_line("#01", "Read").with(code_span("b.txt")));
        assert_eq!(lines[3], outcome_line("✓", "#01 Read: b"));
    }

    /// Shows `lines` as Claude Code wrote them, by the events that claudio reads of tasks and
    /// calls, and gives the lines that come of them.
    fn show_lines(output: &mut Shown, lines: &[String]) -> Vec<Line> {
        for line in lines {
            for event in crate::protocol::read_events(line).expect("a readable line") {
                match event {
                    Event::ToolUse(tool_use) => output.show_tool_use(&tool_use),
                    Event::ToolResult(result) => output.show_tool_result(&result),
                    Event::BackgroundTasks(tasks) => output.work.note_listed(tasks),
                    Event::TaskStarted {
                        task_id,
                        tool_use_id,
                        kind,
                        place,
                        description,
                    } => (output.work).start_task(
                        task_id,
                        tool_use_id.as_deref(),
                        kind,
                        place,
                        &description,
                    ),
                    Event::TaskBackgrounded { task_id } => output.work.move_to_background(&task_id),
                    Event::TaskEnded {
                        task_id,
                        status,
                        summary,
                    } => output.show_task_end(&task_id, status, &summary),
                    _ => {}
                }
            }
        }
        output.take_lines()
    }

    fn running_in_background(tag: &str) -> Line {
        outcome_line("◌", &format!("{tag} Bash: running in background"))
    }

    #[test]
    fn a_background_task_shows_as_it_goes_there_and_as_it_ends() {
        for (status, outcome, word) in [("completed", "✓", "completed"), ("failed", "✗", "failed")]
        {
            let mut output = output_after_message();
            let lines = show_lines(&mut output, &contract::background_task(status));
            assert_eq!(
                lines[2..],
                [
                    running_in_background("#00"),
                    outcome_line(outcome, &format!("#00 Bash: {word}")),
                ]
            );
        }
    }

    #[test]
    fn a_call_moved_to_the_background_shows_as_one_that_went_there() {
        let mut output = output_after_message();
        let lines = show_lines(&mut output, &contract::moved_call());
        assert_eq!(lines[2], running_in_background("#00"));
        assert_eq!(lines[3], outcome_line("✓", "#00 Bash: completed"));
    }

    #[test]
    fn a_task_that_ends_before_its_call_result_shows_its_end_with_the_result() {
        let mut output = output_after_message();
        let lines = contract::background_task("completed");
        // The call, the list, the start, then the end and the empty list before the result
        let early = [&lines[..3], &lines[4..], &lines[3..4]].concat();
        let lines = show_lines(&mut output, &early);
        assert_eq!(
            lines[2..],
            [
                running_in_background("#00"),
                outcome_line("✓", "#00 Bash: completed"),
            ]
        );
        assert!(output.work.task_tag("b040svj56").is_none());
    }

    #[test]
    fn a_task_is_kept_until_it_leaves_the_list_it_was_on() {
        let mut output = output_after_message();
        let moved = contract::moved_call();
        // The call, its start, its move, and its result, without the list that holds the task
        show_lines(&mut output, &[&moved[..2], &moved[3..5]].concat());
        let task = |id: &str| Task {
            id: id.to_string(),
            description: String::new(),
            type_name: "local_bash".to_string(),
        };
        // A list that does not hold the task yet keeps it
        let is_kept = |output: &Shown| output.work.task_tag("b040svj56").is_some();
        output.work.note_listed(vec![task("other")]);
        assert!(is_kept(&output));
        output.work.note_listed(vec![task("b040svj56")]);
        assert!(is_kept(&output));
        // Once it left the list, it ended
        output.work.note_listed(Vec::new());
        assert!(!is_kept(&output));
    }

    #[test]
    fn a_call_that_stops_a_task_names_it_by_its_tag() {
        let mut output = output_after_message();
        let lines = show_lines(&mut output, &contract::stopped_background_task());
        assert_eq!(
            lines[3],
            call_line("#01", "TaskStop").with(Span::new("#00", SUBDUED))
        );
        assert_eq!(lines[4], outcome_line("✗", "#00 Bash: stopped"));
        assert!(lines[5].text().starts_with("✓ #01 TaskStop: "));
    }

    #[test]
    fn only_claude_s_own_calls_in_the_foreground_wait_for_a_move() {
        let mut output = output_after_message();
        let lines = contract::moved_subagent();
        // The subagent's call and start, then its own command's call and start
        show_lines(&mut output, &lines[..4]);
        assert_eq!(output.work.foreground_calls(), ["toolu_agent"]);
        show_lines(&mut output, &lines[4..]);
        assert_eq!(output.work.foreground_calls(), Vec::<String>::new());
    }

    #[test]
    fn a_subagent_shows_as_its_call_and_the_line_of_its_end() {
        for is_backgrounded in [false, true] {
            let mut output = output_after_message();
            let lines = show_lines(&mut output, &contract::subagent(is_backgrounded));
            // After the reply mark, and the call, and its result, for a subagent in the
            // background
            assert!(lines[1].text().starts_with("○ #00 Agent: "));
            assert_eq!(
                lines.last(),
                Some(&outcome_line("✓", "#00 Agent: completed · 1 call"))
            );
            assert_eq!(lines.len(), if is_backgrounded { 4 } else { 3 });
        }
    }

    #[test]
    fn a_subagent_s_calls_and_results_show_in_the_full_view() {
        let lines = full_view_after(|output| {
            show_lines(output, &contract::subagent(false));
        });
        let call = call_line("#00#00", "SubagentHandback").to_string();
        let call = call.trim_end_matches(' ');
        assert!(lines.iter().any(|line| line.starts_with(call)), "{lines:?}");
        let result = outcome_line("✓", "#00#00 SubagentHandback: ").to_string();
        let result = result.trim_end_matches(&PLAIN.code());
        assert!(
            lines.iter().any(|line| line.starts_with(result)),
            "{lines:?}"
        );
    }

    #[test]
    fn a_resumed_subagent_keeps_the_tag_of_the_call_that_first_started_it() {
        let mut output = output_after_message();
        let lines = show_lines(&mut output, &contract::resumed_subagent());
        let send = call_line("#01", "SendMessage").with(Span::new("#00", SUBDUED));
        assert!(lines.contains(&send), "{lines:?}");
        assert_eq!(
            lines.last(),
            Some(&outcome_line("✓", "#01 SendMessage: completed · 1 call"))
        );
    }

    /// The lines of a subagent whose run broke on an API error, in the background when
    /// `is_backgrounded`, as the types of `task_notification` allow for a subagent's task.
    fn failed_subagent(is_backgrounded: bool) -> Vec<String> {
        let input =
            json!({"description": "Fail", "prompt": "Fail", "run_in_background": is_backgrounded});
        let call = json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "c", "name": "Agent", "input": input}]}, "parent_tool_use_id": null});
        let started = json!({"type": "system", "subtype": "task_started", "task_id": "a", "tool_use_id": "c", "description": "Fail", "is_backgrounded": is_backgrounded, "task_type": "local_agent"});
        let error = "Agent terminated early due to an API error";
        let ended = json!({"type": "system", "subtype": "task_notification", "task_id": "a", "tool_use_id": "c", "status": "failed", "summary": error});
        let result = |text: &str, is_error: bool| json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "c", "content": text, "is_error": is_error}]}, "parent_tool_use_id": null});
        let lines = if is_backgrounded {
            vec![
                call,
                started,
                result("Async agent launched successfully.", false),
                ended,
            ]
        } else {
            vec![call, started, ended, result(error, true)]
        };
        lines.iter().map(Value::to_string).collect()
    }

    #[test]
    fn a_message_to_another_than_a_known_subagent_names_whom_it_goes_to() {
        let input = json!({"to": "reviewer", "summary": "Review", "message": "Review the diff"});
        let summary = summarize("SendMessage", input.as_object().expect("an object"));
        assert_eq!(summary.text, "reviewer");
    }

    #[test]
    fn a_subagent_that_failed_says_so_on_the_line_of_its_end() {
        for is_backgrounded in [false, true] {
            let mut output = output_after_message();
            let lines = show_lines(&mut output, &failed_subagent(is_backgrounded));
            assert_eq!(
                lines.last(),
                Some(&outcome_line("✗", "#00 Agent: failed · 0 calls"))
            );
        }
    }

    #[test]
    fn an_agent_call_names_the_subagent_it_started_until_its_result() {
        let mut output = output_after_message();
        let lines = contract::subagent(false);
        // The call and the subagent's start
        show_lines(&mut output, &lines[..2]);
        let Status::Running { call, .. } = output.status() else {
            panic!("Claude runs the Agent call");
        };
        assert_eq!(output.work.name_call("Agent", &call), "Agent #00");
        show_lines(&mut output, &lines[2..]);
        assert_eq!(output.work.name_call("Agent", &call), "Agent");
    }

    #[test]
    fn a_subagent_runs_from_its_task_start_to_its_end() {
        for is_backgrounded in [false, true] {
            let mut output = output_after_message();
            let lines = contract::subagent(is_backgrounded);
            let end = lines
                .iter()
                .position(|line| line.contains("task_notification"))
                .expect("the subagent's end");
            show_lines(&mut output, &lines[..end]);
            let foreground =
                |output: &Shown| output.work.running_subagents(Some(Place::Foreground));
            assert_eq!(output.work.running_subagents(None), 1);
            assert_eq!(foreground(&output), usize::from(!is_backgrounded));
            show_lines(&mut output, &lines[end..]);
            assert_eq!(output.work.running_subagents(None), 0);
            assert_eq!(foreground(&output), 0);
        }
    }

    #[test]
    fn a_task_that_ends_in_the_foreground_shows_as_its_call_result() {
        let mut output = output_after_message();
        let started = json!({"type": "system", "subtype": "task_started", "task_id": "t", "tool_use_id": "c", "description": "Run sleep 4", "is_backgrounded": false});
        let ended = json!({"type": "system", "subtype": "task_notification", "task_id": "t", "status": "completed", "summary": "Run sleep 4"});
        let call = json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "c", "name": "Bash", "input": {"command": "sleep 4", "description": "Run sleep 4"}}]}, "parent_tool_use_id": null});
        let result = json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "c", "content": "", "is_error": false}]}, "parent_tool_use_id": null});
        let lines = [call, started, ended, result].map(|line| line.to_string());
        let lines = show_lines(&mut output, &lines);
        assert_eq!(lines[2..], [outcome_line("✓", "#00 Bash: (no output)")]);
        assert!(output.work.task_tag("t").is_none());
    }

    fn tool_use(name: &str, input: Value) -> ToolUse {
        let Value::Object(input) = input else {
            panic!("an object");
        };
        ToolUse {
            id: "toolu_1".to_string(),
            name: name.to_string(),
            input,
            subagent: None,
        }
    }

    #[test]
    fn a_long_command_shows_its_first_line() {
        let mut output = output_after_message();
        output.show_tool_use(&tool_use(
            "Bash",
            json!({"command": "cd a\nmake\nmake test"}),
        ));
        let lines = output.take_lines();
        assert_eq!(
            lines.last().expect("the call's line"),
            &call_line("#00", "Bash")
                .with(code_span("cd a"))
                .with(" ")
                .with(Span::new("(+2 lines)", SUBDUED))
        );
    }

    #[test]
    fn a_subagent_of_a_type_of_its_own_is_named_by_it() {
        let call = |kind: &str| {
            let input =
                json!({"description": "Review the diff", "prompt": "", "subagent_type": kind});
            summarize("Agent", input.as_object().expect("an object")).text
        };
        assert_eq!(call("code-reviewer"), "code-reviewer · Review the diff");
        assert_eq!(call(DEFAULT_AGENT), "Review the diff");
    }

    #[test]
    fn a_described_call_shows_its_description_in_place_of_its_input() {
        let mut output = output_after_message();
        output.show_tool_use(&tool_use(
            "Bash",
            json!({"command": "cargo test", "description": "Run the tests"}),
        ));
        let lines = output.take_lines();
        assert_eq!(
            lines.last().expect("the call's line"),
            &call_line("#00", "Bash").with("Run the tests")
        );
    }

    #[test]
    fn a_long_input_in_json_is_cut_once() {
        let mut output = output_after_message();
        let message = "x".repeat(300);
        output.show_tool_use(&tool_use("TodoWrite", json!({"message": message})));
        let lines = output.take_lines();
        let line = lines.last().expect("the call's line");
        assert_eq!(line.text().matches('…').count(), 1);
        // The `…` keeps the style of the code it stands for
        assert_eq!(line.characters().last(), Some(('…', CODE)));
    }

    #[test]
    fn a_tool_without_a_summary_field_shows_its_input_in_json_as_code() {
        let mut output = output_after_message();
        output.show_tool_use(&tool_use("TodoWrite", json!({"todos": []})));
        let lines = output.take_lines();
        assert_eq!(
            lines.last().expect("the call's line"),
            &call_line("#00", "TodoWrite").with(code_span(r#"{"todos":[]}"#))
        );
    }

    #[test]
    fn an_edit_shows_how_many_lines_it_changes() {
        let mut output = output_after_message();
        let input = json!({"file_path": "a.rs", "old_string": "x\ny", "new_string": "z"});
        output.show_tool_use(&tool_use("Edit", input));
        let lines = output.take_lines();
        assert_eq!(
            lines.last().expect("the call's line"),
            &call_line("#00", "Edit")
                .with(code_span("a.rs"))
                .with_line(Line::styled([
                    (" ", PLAIN),
                    ("-2", FAILED_OR_REMOVED),
                    (" ", PLAIN),
                    ("+1", DONE_OR_ADDED)
                ]))
        );
    }

    #[test]
    fn tags_wrap_from_ff_to_00() {
        let mut output = output_after_message();
        for id in 0..257 {
            output.show_tool_use(&read_call(&id.to_string(), "a.txt"));
        }
        let lines = output.take_lines();
        assert!(lines[256].text().contains("#ff"));
        assert!(lines[257].text().contains("#00"));
    }

    #[test]
    fn a_text_block_that_never_streamed_shows_whole_at_its_end() {
        let mut output = output_after_message();
        output.show_text_end("Usage");
        assert_eq!(output.take_lines(), [reply_mark(), Line::from("Usage")]);
    }

    #[test]
    fn a_narration_s_mark_says_it_is_a_summary_and_the_next_text_s_does_not() {
        let mut output = output_after_message();
        output.show_narration("Running the tests.");
        output.show_text_end("Done.");
        assert_eq!(
            output.take_lines(),
            [
                summarized_reply_mark(),
                Line::from("Running the tests."),
                Line::default(),
                reply_mark(),
                Line::from("Done.")
            ]
        );
    }

    #[test]
    fn a_narration_ends_at_its_text_since_the_output_sets_blocks_apart() {
        let mut output = output_after_message();
        // The API ends each summary of Claude's thinking with an empty line
        output.show_narration("Running the tests.\n\n");
        output.show_text_end("Done.");
        assert_eq!(
            output.take_lines(),
            [
                summarized_reply_mark(),
                Line::from("Running the tests."),
                Line::default(),
                reply_mark(),
                Line::from("Done.")
            ]
        );
    }

    #[test]
    fn a_streamed_text_block_is_not_repeated_at_its_end() {
        let mut output = output_after_message();
        output.show_text("Hello.");
        output.show_text_end("Hello.");
        assert_eq!(output.take_lines(), [reply_mark(), Line::from("Hello.")]);
    }

    #[test]
    fn a_compaction_answers_the_command_as_claude_code_s_notice() {
        let mut output = output_after_message();
        output.show_command("compact");
        output.show_compacted(19_812, Some(6_578));
        assert_eq!(
            output.take_rows(),
            [
                user_line("/compact"),
                reply_mark().into(),
                Notice::new(NoticeMark::Noted, "compacted from 19,812 to 6,578 tokens").lines()[0]
                    .clone()
                    .into()
            ]
        );
    }

    #[test]
    fn a_long_permission_check_shows_until_it_ends() {
        let mut output = output_after_message();
        output.show_tool_use(&read_call("1", "a.txt"));
        output.note_permission_check("1", true);
        assert!(matches!(output.status(), Status::Checking { tool, .. } if tool == "Read"));
        output.note_permission_check("1", false);
        assert!(matches!(output.status(), Status::Running { tool, .. } if tool == "Read"));
    }

    #[test]
    fn the_permission_check_of_another_call_leaves_the_status() {
        let mut output = output_after_message();
        output.show_tool_use(&read_call("1", "a.txt"));
        output.note_permission_check("1", true);
        output.show_tool_use(&read_call("2", "b.txt"));
        // The check of the first call ends while the second one shows
        output.note_permission_check("1", false);
        assert!(matches!(output.status(), Status::Running { call, .. } if call == "2"));
        output.note_permission_check("2", true);
        output.show_tool_result(&result("2", "b"));
        // The check ends after its call did
        output.note_permission_check("2", false);
        assert!(output.status() == Status::Working);
    }

    #[test]
    fn a_failed_hook_shows_in_red_with_its_error_output() {
        let mut output = output_after_message();
        output.show_hook_failure("Stop", Some(1), "no tmux server\n", false);
        let lines = output.take_lines();
        assert_eq!(
            lines[1..],
            Notice::new(NoticeMark::Error, "Stop hook failed with exit code 1")
                .quoting("no tmux server\n")
                .lines()
        );
    }

    #[test]
    fn a_claude_code_notice_shows_by_its_level_under_a_header_of_claudio_s() {
        let mut output = output_after_message();
        output.show_claude_code_notice("in the full view only", Level::Info);
        output.show_claude_code_notice("quiet", Level::Notice);
        output.show_claude_code_notice("Session resumed", Level::Suggestion);
        output.show_claude_code_notice("loud", Level::Warning);
        let lines = output.take_lines();
        let notes = Notice::new(NoticeMark::Noted, "Claude Code notes");
        let warns = Notice::new(NoticeMark::Warning, "Claude Code warns");
        let expected = [
            notes.quoting("quiet").lines(),
            Notice::new(NoticeMark::Noted, "Claude Code notes")
                .quoting("Session resumed")
                .lines(),
            warns.quoting("loud").lines(),
        ];
        assert_eq!(lines[1..], expected.concat());
    }

    #[test]
    fn an_unknown_kind_shows_once() {
        let mut output = output_after_message();
        output.show_unknown("system message", "api_retry", "{}");
        output.show_unknown("system message", "api_retry", "{}");
        let lines = output.take_lines();
        let warnings = lines
            .iter()
            .filter(|line| line.text().contains("api_retry"));
        assert_eq!(warnings.count(), 1);
    }
}
