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

use crate::markdown::Highlighter;
use crate::protocol::{Ask, AskKind, Question, SettingsError, State, ToolResult, ToolUse};

const GREEN: &str = "\x1b[32m";
// The marks of the user's lines
pub const LIGHT_CYAN: &str = "\x1b[96m";
pub const YELLOW: &str = "\x1b[33m";
// For a warning
pub const ORANGE: &str = "\x1b[38;5;208m";
// Only for failures, since red reads as an error
const RED: &str = "\x1b[31m";
// For what Claude did, which supports what Claude wrote
pub const DIM: &str = "\x1b[2m";
pub const BOLD: &str = "\x1b[1m";
pub const RESET: &str = "\x1b[0m";
/// The background of the user's lines, which sets them apart from Claude's.
pub const BAR_COLOR: u8 = 236;
/// The color of text that stays in the background.
pub const STATUS_COLOR: &str = "\x1b[38;5;244m";
// Erases to the end of the row in the background color that is set, which fills the row's
// rest with the bar without writing spaces into the text. Written after the text too, it fills
// the rest of the last row the terminal wraps the text onto
const FILL_ROW: &str = "\x1b[K";

// Past this, a tool's input in JSON is noise to read through
const INPUT_SUMMARY_CHARACTERS: usize = 200;

/// The field that sums up a call of each tool that has one.
const SUMMARY_FIELDS: [(&str, &str); 10] = [
    ("Agent", "description"),
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
    status: Status,
    /// The tool uses whose result repeats what the screen already shows.
    hidden_results: HashSet<String>,
    /// Each tool use that waits for its result. Claude can run several tools at once, so a
    /// result can come after a line of another tool.
    pending_calls: HashMap<String, PendingCall>,
    /// The columns a detail line takes at most on screen.
    max_detail_columns: NonZeroUsize,
    /// The tag of the next tool use.
    next_tag: u8,
    /// How the user stopped the turn that runs, if they did.
    interruption: Option<Interruption>,
    highlighter: Highlighter,
    /// The lines of Claude Code's error output that wait for Claude to finish the text it
    /// writes.
    waiting_stderr: Vec<String>,
    /// What each task that runs in the background does.
    background_tasks: Vec<String>,
}

/// A tool use that waits for its result, whose line names the tool and repeats the tag.
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
    /// Claude runs the tool with this name.
    Running(String),
}

/// How the user stopped a turn.
#[derive(Clone, Copy)]
pub enum Interruption {
    /// With Ctrl-C, or by refusing a tool, which the turn's end then says.
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
    pub fn new(full_view: File, max_detail_columns: NonZeroUsize) -> Self {
        Output {
            text: String::new(),
            full_view,
            full_view_lines: 0,
            position: Position::Start,
            pending_break: None,
            text_ended: false,
            status: Status::Idle,
            hidden_results: HashSet::new(),
            pending_calls: HashMap::new(),
            max_detail_columns,
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

    pub fn background_tasks(&self) -> &[String] {
        &self.background_tasks
    }

    pub fn note_background_tasks(&mut self, tasks: Vec<String>) {
        self.background_tasks = tasks;
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

    pub fn show_text(&mut self, text: &str) {
        self.begin(Section::Text);
        self.set_status(Status::Writing);
        // The break goes before the text, even before the highlighter settles any of it
        self.write_break();
        let colored = self.highlighter.push(text);
        self.write(&colored);
    }

    pub fn show_text_end(&mut self) {
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
            self.set_status(Status::Running(tool_use.name.clone()));
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
        // One line per call keeps the tool calls out of the way of the conversation
        let summary = summarize(&tool_use.name, &tool_use.input);
        let mut summary_lines = summary.lines();
        let first = summary_lines.next().unwrap_or_default();
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
        self.push_detail(&format!(
            "{mark} {}{RESET} {first}{more}{changes}",
            tool_use.name
        ));
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
        // What a subagent did shows in what it reports back to Claude. A tool the interrupt
        // stopped fails with Claude Code's message to Claude.
        if result.is_subagent
            || self.hidden_results.remove(&result.tool_use_id)
            || (self.interruption.is_some() && result.is_error)
        {
            return;
        }
        self.begin(Section::Tools);
        // The outcome is all the line adds to the call's line, so the rest stays in the back
        let outcome = if result.is_error { "✗" } else { "✓" };
        let text = result.text.as_deref().map_or("(no text)", str::trim);
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
        self.push_detail(&format!("{outcome} {DIM}{label}{first}{more}{RESET}"));
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
            (Some(_), Some(Interruption::Stop)) => Some("Interrupted".to_string()),
            (Some(reason), None) => Some(format!("Turn ended: {reason}")),
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
            let file = error.file.as_deref().unwrap_or("A settings file");
            self.push_detail(&format!(
                "{RED}Skipped {file}, so none of its settings apply: {} {}{RESET}",
                error.path, error.message
            ));
        }
    }

    pub fn show_paying_key(&mut self, key: &str) {
        self.begin(Section::Notice);
        self.push_detail(&format!(
            "{ORANGE}{key} pays for this session, not the subscription{RESET}"
        ));
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

    /// Adds a detail line, after the break that sets it apart, if one waits: cut on screen, since
    /// a detail line can run long, and whole in the full view. A detail line is any line other
    /// than the conversation itself, the user's words and Claude's prose.
    fn push_detail(&mut self, line: &str) {
        self.write_break();
        self.end_claude_line();
        self.text.push_str(&cut(line, self.max_detail_columns));
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

/// The start of a row on the bar, which fills the whole row.
pub fn start_bar_row() -> String {
    format!("\x1b[48;5;{BAR_COLOR}m{FILL_ROW}")
}

/// The marks in front of a message, on a row of their own.
pub fn marks() -> String {
    format!("{LIGHT_CYAN}››")
}

/// The marks in front of what Claude writes, on a row of their own, in the color of the user's
/// marks, since both say who starts.
fn reply_mark() -> String {
    format!("{LIGHT_CYAN}‹‹{RESET}")
}

/// `line` cut in the middle to take at most `columns` columns on screen, with `…` where it is
/// cut. Both its start and its end stay, since the part that tells one line from the next is
/// often at the end, like the file a command ends with:
/// <https://v5-archive.patternfly.org/ux-writing/truncation>.
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
    format!(
        "{}{RESET}…{}{}",
        &line[..start_end],
        colors_at(line, end_start),
        &line[end_start..]
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
    format!("{bar}{YELLOW}{question}{RESET}{bar} {LIGHT_CYAN}›{RESET}{bar} ")
}

/// The input's summary field for a tool that has one, and its start in JSON otherwise.
fn summarize(tool: &str, input: &Map<String, Value>) -> String {
    if let Some((_, field)) = SUMMARY_FIELDS.iter().find(|(name, _)| *name == tool) {
        return read_field(input, field).to_string();
    }
    let json = serde_json::to_string(input).expect("a JSON object turns back into JSON");
    match json.char_indices().nth(INPUT_SUMMARY_CHARACTERS) {
        Some((end, _)) => format!("{DIM}{}…{RESET}", &json[..end]),
        None => format!("{DIM}{json}{RESET}"),
    }
}

fn read_field<'a>(input: &'a Map<String, Value>, field: &str) -> &'a str {
    input.get(field).and_then(Value::as_str).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::config::Config;

    fn new_output() -> Output {
        Output::new(
            tempfile::tempfile().expect("the temporary directory takes a file"),
            Config::default().max_detail_columns,
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
        output.show_text_end();
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
        output.show_text_end();
        output.show_text("Two.");
        output.show_text_end();
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
            Config::default().max_detail_columns,
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
            Config::default().max_detail_columns,
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
                format!("{BOLD}○{RESET} {DIM}#01{RESET}{BOLD} Read{RESET} b.txt"),
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
        assert_eq!(cut("abcdefgh", columns(6)), format!("abc{RESET}…gh"));
    }

    #[test]
    fn the_end_kept_starts_with_the_colors_in_effect_there() {
        assert_eq!(
            cut(&format!("{DIM}abcdef"), columns(4)),
            format!("{DIM}ab{RESET}…{DIM}f")
        );
        assert_eq!(
            cut(&format!("{BOLD}ab{RESET}cdefgh"), columns(4)),
            format!("{BOLD}ab{RESET}…h")
        );
    }

    #[test]
    fn a_wide_character_that_does_not_fit_is_cut_whole() {
        assert_eq!(cut("ab日本", columns(4)), format!("ab{RESET}…"));
    }

    #[test]
    fn a_detail_line_is_cut_on_screen_and_whole_in_the_full_view() {
        let path = "a".repeat(100);
        let mut screen = Vec::new();
        let lines = full_view_after(|output| {
            output.take_lines();
            output.show_tool_use(&read_call("1", &path));
            screen = output.take_lines();
        });
        let call = format!("{BOLD}○{RESET} {DIM}#00{RESET}{BOLD} Read{RESET} {path}");
        assert_eq!(screen[1], cut(&call, Config::default().max_detail_columns));
        assert_ne!(screen[1], call);
        assert_eq!(lines[1], call);
    }

    #[test]
    fn an_error_is_a_detail_line_and_an_answer_stays_whole() {
        let long = "x".repeat(100);
        let mut output = output_after_message();
        output.show_error(&long);
        output.show_answer("Q", &long);
        let lines = output.take_lines();
        assert_eq!(
            lines[1],
            cut(
                &format!("{RED}{long}{RESET}"),
                Config::default().max_detail_columns
            )
        );
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
            format!("{BOLD}○{RESET} {DIM}#01{RESET}{BOLD} Read{RESET} b.txt")
        );
        assert_eq!(lines[3], format!("✓ {DIM}#01 Read: b{RESET}"));
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
                "{BOLD}○{RESET} {DIM}#00{RESET}{BOLD} Bash{RESET} cd a {DIM}(+2 lines){RESET}"
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
                "{BOLD}○{RESET} {DIM}#00{RESET}{BOLD} Edit{RESET} a.rs {RED}-2{RESET} {GREEN}+1{RESET}"
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
}
