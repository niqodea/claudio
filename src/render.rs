//! The conversation as the terminal shows it: plain lines, appended and never redrawn, so the
//! terminal wraps them to its width and copies them whole.
//!
//! The input line stays open below the conversation, so the user can type at any time, and the
//! conversation reaches the terminal above it as text, each line followed by a line break once it
//! ended. Claude's prose reaches it as it streams in, as far as its colors are settled, and the
//! rest of the conversation a whole line at a time.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Write;
use std::num::NonZeroUsize;

use serde_json::{Map, Value};
use unicode_width::UnicodeWidthChar;

use crate::markdown::{Highlighter, code_span};
use crate::protocol::{
    Ask, AskKind, Level, Question, SettingsError, State, Task, TaskStatus, ToolResult, ToolUse,
};
use crate::status::grouped;

use crate::style::{BAR, BOLD, DIM, GREEN, MARK, ORANGE, RED, RESET, STATUS_COLOR};

// Erases to the end of the row in the background color that is set, which fills the row's
// rest with the bar without writing spaces into the text. Written after the text too, it fills
// the rest of the last row the terminal wraps the text onto
const FILL_ROW: &str = "\x1b[K";

/// The tools whose input carries a `description` of the call in Claude's words, which sums it up
/// in place of its input, still whole in the full view.
/// Seen with Claude Code 2.1.322: the field is optional, yet every one of 1,798 Bash calls over
/// 60 days of one user's sessions had it, so a Bash call shows its command only in the full view.
const DESCRIBED_TOOLS: [&str; 2] = ["Agent", "Bash"];

/// The field that sums up a call of each tool that has one, when the call has no description.
const SUMMARY_FIELDS: [(&str, &str); 9] = [
    ("Bash", "command"),
    ("Edit", "file_path"),
    ("ExitPlanMode", "planFilePath"),
    ("Glob", "pattern"),
    ("Grep", "pattern"),
    ("Read", "file_path"),
    ("WebFetch", "url"),
    ("WebSearch", "query"),
    ("Write", "file_path"),
];

/// The conversation's text that waits to be printed above the input line, and what Claude does
/// meanwhile.
pub struct Output {
    /// Each line followed by a line break once it ended. Only Claude's text leaves its last line
    /// open, and any other line ends it first.
    text: String,
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
    /// The kinds of what Claude Code sends that claudio knows nothing of, and said so once.
    unknown_kinds: HashSet<(&'static str, String)>,
    status: Status,
    /// The tool uses whose result repeats what the screen already shows.
    hidden_results: HashSet<String>,
    /// Each tool use that waits for its result. Claude can run several tools at once, so a
    /// result can come after a line of another tool. Every call gets its result, however its
    /// turn ends, and the result removes the call.
    ///
    /// Undocumented: [`crate::contract::a_stopped_call_gets_its_result`]
    pending_calls: HashMap<String, PendingCall>,
    /// The task that each tool use started, by the tool use's id, until the call's result.
    call_tasks: HashMap<String, CallTask>,
    /// Each call whose task goes on in the background once its result came, by the task's id,
    /// so that the task's end repeats the call's tag.
    background_calls: HashMap<String, BackgroundCall>,
    /// The columns a noisy line takes at most on screen.
    max_noisy_line_columns: NonZeroUsize,
    /// The tag of the next tool use.
    next_tag: u8,
    /// How the user stopped the turn that runs, if they did.
    interruption: Option<Interruption>,
    highlighter: Highlighter,
    /// The lines of Claude Code's error output that wait for Claude to finish the text it
    /// writes.
    waiting_stderr: Vec<String>,
    /// The tasks that run in the background, as Claude Code last told them.
    background_tasks: Vec<Task>,
}

/// The task a tool use started, until the call's result.
struct CallTask {
    id: String,
    is_backgrounded: bool,
    /// How the task ended, when it ended before the call's result.
    end: Option<TaskEnd>,
}

struct TaskEnd {
    status: TaskStatus,
    /// Claude Code's summary of the end for Claude.
    summary: String,
}

/// A call whose task runs in the background.
struct BackgroundCall {
    call: PendingCall,
    /// Whether Claude Code listed the task among those that run in the background.
    was_listed: bool,
}

/// A tool use that waits for its result, whose line names the tool and repeats the tag.
#[derive(Clone)]
struct PendingCall {
    name: String,
    /// What ties the result's line to the call's line: two hexadecimal digits, which keep the
    /// columns of both lines in place. Tags count up and wrap from ff to 00, on the assumption
    /// that a call's result arrives before 255 more calls start. A call that waits longer
    /// shares its tag with a newer call.
    tag: u8,
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
    pub fn new(full_view: File, max_noisy_line_columns: NonZeroUsize) -> Self {
        Output {
            text: String::new(),
            full_view,
            full_view_lines: 0,
            position: Position::Start,
            pending_break: None,
            text_ended: false,
            has_streamed: false,
            unknown_kinds: HashSet::new(),
            status: Status::Idle,
            hidden_results: HashSet::new(),
            pending_calls: HashMap::new(),
            call_tasks: HashMap::new(),
            background_calls: HashMap::new(),
            max_noisy_line_columns,
            next_tag: 0,
            interruption: None,
            highlighter: Highlighter::new(),
            waiting_stderr: Vec::new(),
            background_tasks: Vec::new(),
        }
    }

    pub fn status(&self) -> Status {
        self.status.clone()
    }

    pub fn background_tasks(&self) -> &[Task] {
        &self.background_tasks
    }

    /// Notes the tasks that run in the background. A task that an earlier list held and this one
    /// does not has ended, so its call is forgotten, even if its end never came. A task that no
    /// list held yet stays, since it may join a later list.
    pub fn note_background_tasks(&mut self, tasks: Vec<Task>) {
        self.background_calls.retain(|task_id, background| {
            let is_listed = tasks.iter().any(|task| task.id == *task_id);
            let has_left = background.was_listed && !is_listed;
            background.was_listed |= is_listed;
            !has_left
        });
        self.background_tasks = tasks;
    }

    /// Notes that the tool use with `tool_use_id` started the task `task_id`, which runs in the
    /// background when `is_backgrounded`.
    pub fn note_task_started(
        &mut self,
        task_id: String,
        tool_use_id: Option<String>,
        is_backgrounded: bool,
    ) {
        if let Some(tool_use_id) = tool_use_id {
            let task = CallTask {
                id: task_id,
                is_backgrounded,
                end: None,
            };
            self.call_tasks.insert(tool_use_id, task);
        }
    }

    /// Notes that the task `task_id` moved to the background.
    pub fn note_task_backgrounded(&mut self, task_id: &str) {
        if let Some(task) = self.call_task(task_id) {
            task.is_backgrounded = true;
        }
    }

    /// Shows how the task `task_id` ended in the background, under the tag of the call that
    /// started it, or keeps the end for the call's result, when it comes first. A task that ends
    /// in the foreground shows as its call's result.
    pub fn show_task_end(&mut self, task_id: &str, status: TaskStatus, summary: &str) {
        let end = TaskEnd {
            status,
            summary: summary.to_string(),
        };
        if let Some(background) = self.background_calls.remove(task_id) {
            self.begin(Section::Tools);
            self.push_task_end(&background.call, &end);
        } else if let Some(task) = self.call_task(task_id) {
            task.end = Some(end);
        }
    }

    /// The task `task_id` of a call whose result has not come yet.
    fn call_task(&mut self, task_id: &str) -> Option<&mut CallTask> {
        self.call_tasks.values_mut().find(|task| task.id == task_id)
    }

    /// Adds the line of how the task of `call` ended, with Claude Code's summary for Claude in
    /// the full view.
    fn push_task_end(&mut self, call: &PendingCall, end: &TaskEnd) {
        let (outcome, word) = match end.status {
            TaskStatus::Completed => ("✓", "completed"),
            TaskStatus::Failed => ("✗", "failed"),
            TaskStatus::Stopped => ("✗", "stopped"),
        };
        self.push_detail(&format!(
            "{outcome} {DIM}#{:02x} {}: {word}{RESET}",
            call.tag, call.name
        ));
        self.write_full_view_line(&format!("  {}", end.summary));
    }

    /// Takes the text that waits to be printed above the input line.
    pub fn take_text(&mut self) -> String {
        std::mem::take(&mut self.text)
    }

    #[cfg(test)]
    fn take_lines(&mut self) -> Vec<String> {
        self.take_text().lines().map(str::to_string).collect()
    }

    /// Shows the message the user sends, after all that Claude wrote before it, so the
    /// conversation reads in the order Claude sees it.
    pub fn show_message(&mut self, text: &str) {
        // A message follows the line before it, and starts what the reply mark then introduces
        self.pending_break = None;
        // Each message starts with marks of its own, as Claude Code keeps each message apart, also
        // those it delivers together in one turn
        self.emit(&format!("{}{}{FILL_ROW}{RESET}", start_bar_row(), marks()));
        for line in text.split('\n') {
            self.emit(&format!("{}{line}{FILL_ROW}{RESET}", start_bar_row()));
        }
        self.position = Position::In(Section::Message);
        self.set_status(Status::Working);
        // An interrupt sent as its turn ended had no turn left to stop
        self.interruption = None;
    }

    /// Marks where the context was cleared, which ended the session with `previous_session`, so
    /// the conversation reads as a new one from there.
    pub fn show_reset(&mut self, previous_session: &str) {
        self.show_user_line(&format!("cleared │ session {previous_session}"));
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
        self.emit(&user_line(what));
        self.position = Position::In(Section::Message);
    }

    /// Notes that Claude Code compacted the context, from `before` tokens to `after`, when known.
    pub fn show_compacted(&mut self, before: u64, after: Option<u64>) {
        let after = after.map_or_else(String::new, |after| format!(" to {}", grouped(after)));
        self.begin(Section::Notice);
        self.push_detail(&format!(
            "{DIM}compacted from {}{after} tokens{RESET}",
            grouped(before)
        ));
    }

    /// Says once for each kind that claudio skips what Claude Code sent, `name` of `part`, which
    /// claudio knows nothing of, so the user learns that Claude Code may show more than claudio.
    /// The full view holds the `line` that carried it.
    pub fn show_unknown(&mut self, part: &'static str, name: &str, line: &str) {
        if !self.unknown_kinds.insert((part, name.to_string())) {
            return;
        }
        self.begin(Section::Notice);
        self.push_detail(&format!(
            "{ORANGE}Claude Code's {name} {part}s are not shown: Ctrl+O shows the first{RESET}"
        ));
        self.write_full_view_line(&format!("  {line}"));
    }

    /// Shows the user's answer to what a tool asks, after the question.
    pub fn show_answer(&mut self, question: &str, text: &str) {
        self.push(&format!(
            "{}{text}{FILL_ROW}{RESET}",
            answer_label(question)
        ));
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
        self.write(&colored);
    }

    /// Ends a block of Claude's text, whose whole `text` shows here when none of it streamed,
    /// as for a command's output, which Claude Code writes itself.
    pub fn show_text_end(&mut self, text: &str) {
        if !std::mem::take(&mut self.has_streamed) && !text.is_empty() {
            self.show_text(text);
            self.has_streamed = false;
        }
        self.end_text();
        self.text_ended = true;
        self.set_status(Status::Working);
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
            self.begin(Section::Notice);
            self.push_detail(&format!("{DIM}{exit}{RESET}"));
        }
    }

    /// Adds the rest of the text Claude wrote, which the highlighter holds.
    fn end_text(&mut self) {
        let colored = self.highlighter.finish();
        self.write(&colored);
    }

    pub fn show_tool_use(&mut self, tool_use: &ToolUse) {
        if !tool_use.is_subagent {
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
        // What a subagent did shows in what it reports back to Claude, so its calls have no
        // result line to tie to
        let mark = if tool_use.is_subagent {
            format!("  {BOLD}○")
        } else {
            let tag = self.next_tag;
            self.next_tag = tag.wrapping_add(1);
            self.pending_calls.insert(
                tool_use.id.clone(),
                PendingCall {
                    name: tool_use.name.clone(),
                    tag,
                },
            );
            format!("{BOLD}○{RESET} {DIM}#{tag:02x}{RESET}{BOLD}")
        };
        self.begin(Section::Tools);
        // One line per call keeps the tool calls out of the way of the conversation. A call that
        // names a task, like TaskStop's, names it by the tag of the call that started it
        let task_call = (self.background_calls).get(read_field(&tool_use.input, "task_id"));
        let summary = match task_call {
            Some(BackgroundCall { call, .. }) => Summary {
                text: format!("{DIM}#{:02x}{RESET}", call.tag),
                is_input: false,
            },
            None => summarize(&tool_use.name, &tool_use.input),
        };
        let mut summary_lines = summary.text.lines();
        let first = summary_lines.next().unwrap_or_default();
        let first = if summary.is_input {
            code_span(first)
        } else {
            first.to_string()
        };
        let more = match summary_lines.count() {
            0 => String::new(),
            count => format!(" {DIM}(+{count} lines){RESET}"),
        };
        let changes = if tool_use.name == "Edit" {
            let removed = read_field(&tool_use.input, "old_string").lines().count();
            let added = read_field(&tool_use.input, "new_string").lines().count();
            format!(" {RED}-{removed}{RESET} {GREEN}+{added}{RESET}")
        } else {
            String::new()
        };
        let line = format!("{mark} {}{RESET} {first}{more}{changes}", tool_use.name);
        // A description is Claude's sentence about the call, so only the call's input is noisy
        if summary.is_input {
            self.push_noisy(&line);
        } else {
            self.push_detail(&line);
        }
        let indent = if tool_use.is_subagent { "    " } else { "  " };
        for (field, value) in &tool_use.input {
            match value {
                Value::String(text) => {
                    self.write_full_view_line(&format!("{indent}{DIM}{field}:{RESET}"));
                    for line in text.lines() {
                        self.write_full_view_line(&format!("{indent}  {line}"));
                    }
                }
                _ => self.write_full_view_line(&format!("{indent}{DIM}{field}:{RESET} {value}")),
            }
        }
    }

    pub fn show_tool_result(&mut self, result: &ToolResult) {
        if !result.is_subagent {
            self.set_status(Status::Working);
        }
        let call = self.pending_calls.remove(&result.tool_use_id);
        let task = self.call_tasks.remove(&result.tool_use_id);
        // What a subagent did shows in what it reports back to Claude. A tool the interrupt
        // stopped fails with Claude Code's message to Claude.
        if result.is_subagent
            || self.hidden_results.remove(&result.tool_use_id)
            || (self.interruption.is_some() && result.is_error)
        {
            return;
        }
        self.begin(Section::Tools);
        let text = result.text.as_deref().map_or("(no text)", str::trim);
        // A call whose task goes on in the background has only started, and its result is
        // Claude Code's note to Claude with the task's id and output file, so the line says that
        // the task runs, and the task's end shows the outcome
        if let (Some(call), Some(task)) = (&call, task)
            && task.is_backgrounded
        {
            self.push_detail(&format!(
                "◌ {DIM}#{:02x} {}: running in background{RESET}",
                call.tag, call.name
            ));
            for line in text.lines() {
                self.write_full_view_line(&format!("  {line}"));
            }
            if let Some(end) = task.end {
                self.push_task_end(call, &end);
            } else {
                let was_listed = self.background_tasks.iter().any(|old| old.id == task.id);
                let background = BackgroundCall {
                    call: call.clone(),
                    was_listed,
                };
                self.background_calls.insert(task.id, background);
            }
            return;
        }
        // The outcome is all the line adds to the call's line, so the rest stays in the back
        let outcome = if result.is_error { "✗" } else { "✓" };
        let mut lines = text.lines();
        let first = lines.next().unwrap_or("(no output)");
        let rest = lines.count();
        let more = match rest {
            0 => String::new(),
            count => format!(" (+{count} lines)"),
        };
        // Other lines can come between a call and its result, so the result stands on its own
        let label = call.map_or_else(String::new, |call| {
            format!("#{:02x} {}: ", call.tag, call.name)
        });
        self.push_noisy(&format!("{outcome} {DIM}{label}{first}{more}{RESET}"));
        for line in text.lines().skip(1) {
            self.write_full_view_line(&format!("  {line}"));
        }
    }

    /// Shows what `ask` asks, and gives the line of the full view, from 1, that each of its
    /// questions starts on.
    pub fn show_ask(&mut self, ask: &Ask) -> Vec<usize> {
        self.begin(Section::Tools);
        match &ask.kind {
            // Refused as it arrives, which the error line says
            AskKind::Permission => vec![],
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
            (Some(reason), None) => Some(format!("turn ended: {reason}")),
        };
        if let Some(line) = line {
            self.begin(Section::Notice);
            self.push_detail(&format!("{DIM}{line}{RESET}"));
        }
        // A message that waits for its reply still waits when an older turn ends
        if self.position != Position::In(Section::Message) {
            self.position = Position::TurnEnd;
        }
    }

    pub fn show_settings_errors(&mut self, errors: &[SettingsError]) {
        self.begin(Section::Notice);
        for error in errors {
            let file = error.file.as_deref().unwrap_or("a settings file");
            self.push_detail(&format!(
                "{RED}Claude Code skipped {file}, so none of its settings apply: {} {}{RESET}",
                error.path, error.message
            ));
        }
    }

    /// Warns that claudio runs a Claude Code other than the one it was checked against, whose
    /// changes may break what claudio assumes, like the shape of what it writes.
    pub fn show_unchecked_version(&mut self, version: &str) {
        self.begin(Section::Notice);
        self.push_detail(&format!(
            "{ORANGE}Claude Code {version} runs, but {} is the one checked to work{RESET}",
            crate::claude_code::VERSION
        ));
    }

    /// Warns that a resize can leave copies of the live rows in the scrollback, since frames can
    /// only count their way back to the first of them in this terminal.
    pub fn show_inexact_redraw(&mut self) {
        self.begin(Section::Notice);
        self.push_detail(&format!(
            "{ORANGE}a resize in this terminal can leave copies of the label row and the box in the scrollback, which tmux 3.5 avoids{RESET}"
        ));
    }

    pub fn show_paying_key(&mut self, key: &str) {
        self.begin(Section::Notice);
        self.push_detail(&format!(
            "{ORANGE}{key} pays for this session, not the subscription{RESET}"
        ));
    }

    /// Shows that the hook `hook` failed with `exit_code`, or was cancelled, when
    /// `is_cancelled`, then its error output.
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
        self.show_error(&format!("{hook} hook {how}"));
        for line in stderr.lines().filter(|line| !line.trim().is_empty()) {
            self.push_stderr(line);
        }
    }

    /// Shows what Claude Code tells the user at `level`: in the full view only when it is
    /// `Info`, dim when it is a `Notice`, and else in orange, after the level's name.
    pub fn show_notice(&mut self, text: &str, level: Level) {
        let lines: Vec<String> = match level {
            Level::Info => {
                for line in text.lines() {
                    self.write_full_view_line(line);
                }
                return;
            }
            Level::Notice => text
                .lines()
                .map(|line| format!("{DIM}{line}{RESET}"))
                .collect(),
            Level::Suggestion | Level::Warning => {
                let name = if level == Level::Warning {
                    "warning"
                } else {
                    "suggestion"
                };
                text.lines()
                    .enumerate()
                    .map(|(index, line)| match index {
                        0 => format!("{ORANGE}{name}: {line}{RESET}"),
                        _ => format!("{ORANGE}{line}{RESET}"),
                    })
                    .collect()
            }
        };
        self.begin(Section::Notice);
        for line in lines {
            self.push_detail(&line);
        }
    }

    pub fn show_error(&mut self, error: &str) {
        self.begin(Section::Notice);
        self.push_detail(&format!("{RED}{error}{RESET}"));
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
        self.begin(Section::Notice);
        self.push_detail(&format!("{DIM}stderr │{RESET} {RED}{line}{RESET}"));
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

    /// Adds a whole line, after the break that sets it apart, if one waits.
    fn push(&mut self, line: &str) {
        self.write_break();
        self.emit(line);
    }

    fn write_break(&mut self) {
        match self.pending_break.take() {
            Some(Break::Reply) => self.emit(&reply_mark()),
            Some(Break::Prose) => {
                self.emit("");
                self.emit(&reply_mark());
            }
            Some(Break::Blank) => self.emit(""),
            None => {}
        }
    }

    /// Adds a detail line, after the break that sets it apart, if one waits. A detail line is any
    /// line other than the conversation itself, the user's words and Claude's prose.
    fn push_detail(&mut self, line: &str) {
        self.write_break();
        self.emit(line);
    }

    /// Adds a noisy detail line: cut on screen, and whole in the full view. A noisy line shows a
    /// tool's input or output as it is, so it can run to hundreds of columns, while its start
    /// and end tell it from the next.
    ///
    /// The cut here is the only one a noisy line gets, so callers pass their text whole, and the
    /// full view shows it as it was.
    fn push_noisy(&mut self, line: &str) {
        self.write_break();
        self.end_claude_line();
        self.text.push_str(&cut(line, self.max_noisy_line_columns));
        self.text.push('\n');
        self.write_full_view_line(line);
    }

    /// Adds a whole line to the screen and to the full view, on a row of its own.
    fn emit(&mut self, line: &str) {
        self.end_claude_line();
        self.write(&format!("{line}\n"));
    }

    /// Ends Claude's line where Claude got to, as when a message interrupts Claude mid-line.
    fn end_claude_line(&mut self) {
        let colored = self.highlighter.end_line();
        self.write(&colored);
    }

    /// Adds text to the screen and to the full view.
    fn write(&mut self, text: &str) {
        self.write_full_view_text(text);
        self.text.push_str(text);
    }

    /// Adds a line to the full view only.
    fn write_full_view_line(&mut self, line: &str) {
        self.write_full_view_text(&format!("{line}\n"));
    }

    /// Adds text to the full view, without the codes that fill a row with the bar, since a pager
    /// shows only the codes of colors. A write that fails, like on a full disk, leaves the text
    /// out of the full view only.
    fn write_full_view_text(&mut self, text: &str) {
        self.full_view_lines += text.matches('\n').count();
        let _ = self
            .full_view
            .write_all(text.replace(FILL_ROW, "").as_bytes());
    }

    /// Adds detail lines, and gives the line of the full view, from 1, that the first one is on.
    fn push_details(&mut self, lines: Vec<String>) -> usize {
        self.write_break();
        self.end_claude_line();
        let first = self.full_view_lines + 1;
        for line in lines {
            self.push_detail(&line);
        }
        first
    }
}

/// A line of what the user did, like a command or a clear, on the bar, where the user's lines
/// are, in the style of the label row's state. Only the user's lines are on the bar.
pub fn user_line(what: &str) -> String {
    format!("{}{} {STATUS_COLOR}{what}{RESET}", start_bar_row(), marks())
}

/// The start of a row on the bar, which fills the whole row.
pub fn start_bar_row() -> String {
    format!("{BAR}{FILL_ROW}")
}

/// The marks in front of a message, on a row of their own.
pub fn marks() -> String {
    format!("{MARK}››")
}

/// The marks in front of what Claude writes, on a row of their own, in the color of the user's
/// marks, since both say who starts.
fn reply_mark() -> String {
    format!("{MARK}‹‹{RESET}")
}

/// `line` cut in the middle to take at most `columns` columns on screen, with `…` where it is
/// cut. Both its start and its end stay, since the part that tells one line from the next is
/// often at the end, like the file a command ends with:
/// <https://v5-archive.patternfly.org/ux-writing/truncation>.
/// The line holds its own colors, since the `…` and the end take the colors in effect where the
/// line has them, and know nothing of colors set before it.
pub fn cut(line: &str, columns: NonZeroUsize) -> String {
    let mut characters = Vec::new();
    let mut rest = line.char_indices();
    while let Some((index, character)) = rest.next() {
        if character == '\x1b' {
            // Every sequence a line holds is a CSI, which ends with a letter and takes no column
            rest.find(|(_, next)| next.is_ascii_alphabetic());
            continue;
        }
        characters.push((index, character, character.width().unwrap_or(0)));
    }
    if characters.iter().map(|(_, _, width)| width).sum::<usize>() <= columns.get() {
        return line.to_string();
    }
    // The `…` takes a column, and the start takes the odd one of the rest
    let kept_columns = columns.get() - 1;
    let (mut start_end, mut start_width) = (0, 0);
    for (index, character, width) in &characters {
        if start_width + width > kept_columns.div_ceil(2) {
            break;
        }
        start_width += width;
        start_end = index + character.len_utf8();
    }
    let (mut end_start, mut end_width) = (line.len(), 0);
    for (index, _, width) in characters.iter().rev() {
        if end_width + width > kept_columns / 2 {
            break;
        }
        end_width += width;
        end_start = *index;
    }
    // The `…` stands for the text it replaces, so it takes the colors of that text's start
    let cut_start = characters
        .iter()
        .find(|(index, _, _)| *index >= start_end)
        .map_or(start_end, |(index, _, _)| *index);
    let end = &line[end_start..];
    let end = if end.is_empty() {
        String::new()
    } else {
        format!("{}{end}", colors_at(line, end_start))
    };
    format!(
        "{}{RESET}{}…{RESET}{end}",
        &line[..start_end],
        colors_at(line, cut_start),
    )
}

/// The color codes in effect at byte `end` of `line`, those since its last reset, which set the
/// colors again where printing starts there.
pub fn colors_at(line: &str, end: usize) -> String {
    let mut codes = String::new();
    let mut characters = line[..end].char_indices();
    while let Some((index, character)) = characters.next() {
        if character != '\x1b' {
            continue;
        }
        // Every sequence a line holds is a CSI, which ends with a letter, and those that end with
        // `m` set colors
        let sequence_end = characters
            .find(|(_, next)| next.is_ascii_alphabetic())
            .map_or(end, |(next_index, next)| next_index + next.len_utf8());
        let sequence = &line[index..sequence_end];
        if sequence == RESET {
            codes.clear();
        } else if sequence.ends_with('m') {
            codes.push_str(sequence);
        }
    }
    codes
}

/// The lines of Markdown, highlighted.
pub fn markdown_lines(markdown: &str) -> Vec<String> {
    let mut highlighter = Highlighter::new();
    let mut colored = highlighter.push(markdown.trim());
    colored.push_str(&highlighter.finish());
    colored.lines().map(str::to_string).collect()
}

/// The lines of a question: its header and text, then its options, numbered from 1 as the question
/// view numbers them.
pub fn question_lines(question: &Question) -> Vec<String> {
    let mut lines = vec![format!(
        "{BOLD}{}{RESET} {}",
        question.header, question.text
    )];
    for (number, choice) in (1..).zip(&question.options) {
        lines.push(format!(
            "  {number}. {} {DIM}{}{RESET}",
            choice.label, choice.description
        ));
        if let Some(preview) = &choice.preview {
            for line in preview.lines() {
                lines.push(format!("     {DIM}{line}{RESET}"));
            }
        }
    }
    lines
}

/// The question in front of an answer, on the bar.
pub fn answer_label(question: &str) -> String {
    let bar = start_bar_row();
    format!("{bar}{BOLD}{question}{RESET}{bar} {MARK}›{RESET}{bar} ")
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
        return Summary {
            text: description.to_string(),
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

    fn new_output() -> Output {
        Output::new(
            tempfile::tempfile().expect("the temporary directory takes a file"),
            Config::default().max_noisy_line_columns,
        )
    }

    fn output_after_message() -> Output {
        let mut output = new_output();
        output.show_message("Hi");
        output.take_lines();
        output
    }

    #[test]
    fn a_reply_starts_with_its_mark() {
        let mut output = output_after_message();
        output.show_text("Hello.\n");
        assert_eq!(output.take_lines(), [reply_mark(), "Hello.".to_string()]);
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
                "One.".to_string(),
                String::new(),
                "Two.".to_string()
            ]
        );
        assert_eq!(lines[5], format!("{DIM}stderr │{RESET} {RED}Failed{RESET}"));
    }

    #[test]
    fn a_message_sent_mid_line_follows_what_claude_wrote_of_the_line() {
        let mut output = output_after_message();
        output.show_text("Hel");
        output.show_message("Stop");
        let lines = output.take_lines();
        assert_eq!(lines[..2], [reply_mark(), "Hel".to_string()]);
        assert!(lines[3].ends_with(&format!("Stop{FILL_ROW}{RESET}")));
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
                "Hel".to_string(),
                String::new(),
                format!("{RED}Failed{RESET}")
            ]
        );
    }

    #[test]
    fn a_line_start_the_highlighter_holds_follows_the_mark() {
        let mut output = output_after_message();
        output.show_text("##");
        output.show_message("Stop");
        let lines = output.take_lines();
        assert_eq!(lines[..2], [reply_mark(), "##".to_string()]);
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
            is_subagent: false,
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
        assert_eq!(output.take_lines(), Vec::<String>::new());
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
                "Hi.".to_string(),
                String::new(),
                reply_mark(),
                "Done.".to_string()
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
            [String::new(), reply_mark(), "Read it.".to_string()]
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
                "One.".to_string(),
                String::new(),
                reply_mark(),
                "Two.".to_string()
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
        assert_eq!(output.take_lines(), [reply_mark(), "Stopped.".to_string()]);
    }

    /// Reads back what the output wrote to the full view, after the user's first message.
    fn full_view_after(show: impl FnOnce(&mut Output)) -> Vec<String> {
        let file = tempfile::NamedTempFile::new().expect("the temporary directory takes a file");
        let mut output = Output::new(
            file.reopen().expect("the file opens again"),
            Config::default().max_noisy_line_columns,
        );
        output.show_message("Hi");
        show(&mut output);
        let text = std::fs::read_to_string(file.path()).expect("the full view reads back");
        text.lines().skip(2).map(str::to_string).collect()
    }

    #[test]
    fn each_question_starts_on_the_full_view_line_its_ask_gives() {
        let file = tempfile::NamedTempFile::new().expect("the temporary directory takes a file");
        let mut output = Output::new(
            file.reopen().expect("the file opens again"),
            Config::default().max_noisy_line_columns,
        );
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
        assert_eq!(
            headers,
            [
                format!("{BOLD}Database{RESET} Which database?"),
                format!("{BOLD}Checks{RESET} Which checks?"),
            ]
        );
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
                is_subagent: false,
            });
        });
        assert_eq!(
            lines[2..],
            [
                format!("  {DIM}command:{RESET}"),
                "    cd a".to_string(),
                "    make".to_string(),
                format!("  {DIM}timeout:{RESET} 5"),
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
                format!("✓ {DIM}#00 Read: one (+1 lines){RESET}"),
                "  two".to_string(),
                format!(
                    "{BOLD}○{RESET} {DIM}#01{RESET}{BOLD} Read{RESET} {}",
                    code_span("b.txt")
                ),
                format!("  {DIM}file_path:{RESET}"),
                "    b.txt".to_string(),
                format!("✓ {DIM}#01 Read: three{RESET}"),
            ]
        );
    }

    fn columns(count: usize) -> NonZeroUsize {
        NonZeroUsize::new(count).expect("the count is not zero")
    }

    #[test]
    fn a_line_that_fits_stays_whole() {
        assert_eq!(cut("abc", columns(3)), "abc");
    }

    #[test]
    fn a_line_too_long_keeps_its_start_and_its_end() {
        assert_eq!(cut("abcdefgh", columns(6)), format!("abc{RESET}…{RESET}gh"));
    }

    #[test]
    fn the_end_kept_starts_with_the_colors_in_effect_there() {
        assert_eq!(
            cut(&format!("{DIM}abcdef"), columns(4)),
            format!("{DIM}ab{RESET}{DIM}…{RESET}{DIM}f")
        );
        assert_eq!(
            cut(&format!("{BOLD}ab{RESET}cdefgh"), columns(4)),
            format!("{BOLD}ab{RESET}…{RESET}h")
        );
    }

    #[test]
    fn the_ellipsis_takes_the_colors_of_the_text_it_replaces() {
        assert_eq!(
            cut(&format!("ab{DIM}cdefgh"), columns(4)),
            format!("ab{RESET}{DIM}…{RESET}{DIM}h")
        );
    }

    #[test]
    fn a_wide_character_that_does_not_fit_is_cut_whole() {
        assert_eq!(cut("ab日本", columns(4)), format!("ab{RESET}…{RESET}"));
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
        let call = format!(
            "{BOLD}○{RESET} {DIM}#00{RESET}{BOLD} Read{RESET} {}",
            code_span(&path)
        );
        assert_eq!(
            screen[1],
            cut(&call, Config::default().max_noisy_line_columns)
        );
        assert_ne!(screen[1], call);
        assert_eq!(lines[1], call);
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
            is_subagent: false,
        });
        assert!(output.take_lines()[1].contains(&description));
    }

    #[test]
    fn an_error_and_an_answer_stay_whole() {
        let long = "x".repeat(100);
        let mut output = output_after_message();
        output.show_error(&long);
        output.show_answer("Q", &long);
        let lines = output.take_lines();
        assert_eq!(lines[1], format!("{RED}{long}{RESET}"));
        assert!(lines[2].contains(&long));
    }

    #[test]
    fn the_full_view_leaves_out_the_codes_that_fill_a_row() {
        let lines = full_view_after(|_| {});
        assert!(!lines.join("").contains(FILL_ROW));
    }

    fn read_call(id: &str, path: &str) -> ToolUse {
        let mut input = Map::new();
        input.insert("file_path".to_string(), Value::String(path.to_string()));
        ToolUse {
            id: id.to_string(),
            name: "Read".to_string(),
            input,
            is_subagent: false,
        }
    }

    fn result(id: &str, text: &str) -> ToolResult {
        ToolResult {
            tool_use_id: id.to_string(),
            text: Some(text.to_string()),
            is_error: false,
            is_subagent: false,
        }
    }

    #[test]
    fn a_result_repeats_its_call_tag_and_names_its_tool() {
        let mut output = output_after_message();
        output.show_tool_use(&read_call("1", "a.txt"));
        output.show_tool_use(&read_call("2", "b.txt"));
        output.show_tool_result(&result("2", "b"));
        let lines = output.take_lines();
        assert_eq!(
            lines[2],
            format!(
                "{BOLD}○{RESET} {DIM}#01{RESET}{BOLD} Read{RESET} {}",
                code_span("b.txt")
            )
        );
        assert_eq!(lines[3], format!("✓ {DIM}#01 Read: b{RESET}"));
    }

    /// Shows `lines` as Claude Code wrote them, by the events that claudio reads of tasks and
    /// calls, and gives the lines that come of them.
    fn show_lines(output: &mut Output, lines: &[String]) -> Vec<String> {
        for line in lines {
            for event in crate::protocol::read_events(line).expect("a readable line") {
                match event {
                    Event::ToolUse(tool_use) => output.show_tool_use(&tool_use),
                    Event::ToolResult(result) => output.show_tool_result(&result),
                    Event::BackgroundTasks(tasks) => output.note_background_tasks(tasks),
                    Event::TaskStarted {
                        task_id,
                        tool_use_id,
                        is_backgrounded,
                    } => output.note_task_started(task_id, tool_use_id, is_backgrounded),
                    Event::TaskBackgrounded { task_id } => output.note_task_backgrounded(&task_id),
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

    fn running_in_background(tag: &str) -> String {
        format!("◌ {DIM}#{tag} Bash: running in background{RESET}")
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
                    running_in_background("00"),
                    format!("{outcome} {DIM}#00 Bash: {word}{RESET}"),
                ]
            );
        }
    }

    #[test]
    fn a_call_moved_to_the_background_shows_as_one_that_went_there() {
        let mut output = output_after_message();
        let lines = show_lines(&mut output, &contract::moved_call());
        assert_eq!(lines[2], running_in_background("00"));
        assert_eq!(lines[3], format!("✓ {DIM}#00 Bash: completed{RESET}"));
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
                running_in_background("00"),
                format!("✓ {DIM}#00 Bash: completed{RESET}"),
            ]
        );
        assert!(output.background_calls.is_empty());
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
        };
        // A list that does not hold the task yet keeps it
        output.note_background_tasks(vec![task("other")]);
        assert_eq!(output.background_calls.len(), 1);
        output.note_background_tasks(vec![task("b040svj56")]);
        assert_eq!(output.background_calls.len(), 1);
        // Once it left the list, it ended
        output.note_background_tasks(Vec::new());
        assert!(output.background_calls.is_empty());
    }

    #[test]
    fn a_call_that_stops_a_task_names_it_by_its_tag() {
        let mut output = output_after_message();
        let lines = show_lines(&mut output, &contract::stopped_background_task());
        assert_eq!(
            lines[3],
            format!("{BOLD}○{RESET} {DIM}#01{RESET}{BOLD} TaskStop{RESET} {DIM}#00{RESET}")
        );
        assert_eq!(lines[4], format!("✗ {DIM}#00 Bash: stopped{RESET}"));
        assert!(lines[5].starts_with(&format!("✓ {DIM}#01 TaskStop: ")));
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
        assert_eq!(lines[2..], [format!("✓ {DIM}#00 Bash: (no output){RESET}")]);
        assert!(output.call_tasks.is_empty());
    }

    fn tool_use(name: &str, input: Value) -> ToolUse {
        let Value::Object(input) = input else {
            panic!("an object");
        };
        ToolUse {
            id: "toolu_1".to_string(),
            name: name.to_string(),
            input,
            is_subagent: false,
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
            &format!(
                "{BOLD}○{RESET} {DIM}#00{RESET}{BOLD} Bash{RESET} {} {DIM}(+2 lines){RESET}",
                code_span("cd a")
            )
        );
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
            &format!("{BOLD}○{RESET} {DIM}#00{RESET}{BOLD} Bash{RESET} Run the tests")
        );
    }

    #[test]
    fn a_long_input_in_json_is_cut_once() {
        let mut output = output_after_message();
        let message = "x".repeat(300);
        output.show_tool_use(&tool_use("TodoWrite", json!({"message": message})));
        let lines = output.take_lines();
        let line = lines.last().expect("the call's line");
        assert_eq!(line.matches('…').count(), 1);
        assert!(line.ends_with(&format!("x\"}}`{RESET}")));
    }

    #[test]
    fn a_tool_without_a_summary_field_shows_its_input_in_json_as_code() {
        let mut output = output_after_message();
        output.show_tool_use(&tool_use("TodoWrite", json!({"todos": []})));
        let lines = output.take_lines();
        assert_eq!(
            lines.last().expect("the call's line"),
            &format!(
                "{BOLD}○{RESET} {DIM}#00{RESET}{BOLD} TodoWrite{RESET} {}",
                code_span(r#"{"todos":[]}"#)
            )
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
            &format!(
                "{BOLD}○{RESET} {DIM}#00{RESET}{BOLD} Edit{RESET} {} {RED}-2{RESET} {GREEN}+1{RESET}",
                code_span("a.rs")
            )
        );
    }

    #[test]
    fn tags_wrap_from_ff_to_00() {
        let mut output = output_after_message();
        for id in 0..257 {
            output.show_tool_use(&read_call(&id.to_string(), "a.txt"));
        }
        let lines = output.take_lines();
        assert!(lines[256].contains("#ff"));
        assert!(lines[257].contains("#00"));
    }

    #[test]
    fn a_text_block_that_never_streamed_shows_whole_at_its_end() {
        let mut output = output_after_message();
        output.show_text_end("Usage");
        assert_eq!(output.take_lines(), [reply_mark(), "Usage".to_string()]);
    }

    #[test]
    fn a_streamed_text_block_is_not_repeated_at_its_end() {
        let mut output = output_after_message();
        output.show_text("Hello.");
        output.show_text_end("Hello.");
        assert_eq!(output.take_lines(), [reply_mark(), "Hello.".to_string()]);
    }

    #[test]
    fn a_compaction_answers_the_command_as_claude_code_s_notice() {
        let mut output = output_after_message();
        output.show_command("compact");
        output.show_compacted(19_812, Some(6_578));
        assert_eq!(
            output.take_lines(),
            [
                user_line("/compact"),
                reply_mark(),
                format!("{DIM}compacted from 19,812 to 6,578 tokens{RESET}")
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
            lines[1],
            format!("{RED}Stop hook failed with exit code 1{RESET}")
        );
        assert!(lines[2].contains("stderr │") && lines[2].contains("no tmux server"));
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn a_notice_shows_by_its_level() {
        let mut output = output_after_message();
        output.show_notice("in the full view only", Level::Info);
        output.show_notice("quiet", Level::Notice);
        output.show_notice("loud", Level::Warning);
        let lines = output.take_lines();
        assert!(!lines.join("").contains("in the full view only"));
        assert_eq!(lines[1], format!("{DIM}quiet{RESET}"));
        assert_eq!(lines[2], format!("{ORANGE}warning: loud{RESET}"));
    }

    #[test]
    fn an_unknown_kind_shows_once() {
        let mut output = output_after_message();
        output.show_unknown("system message", "api_retry", "{}");
        output.show_unknown("system message", "api_retry", "{}");
        let lines = output.take_lines();
        let warnings = lines.iter().filter(|line| line.contains("api_retry"));
        assert_eq!(warnings.count(), 1);
    }
}
