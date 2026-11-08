//! The bottom of the terminal: the label row, which shows the context, the usage limits and what
//! Claude does, and below it the input box, where the user types with vi keys. The conversation
//! scrolls above them, so the lines that leave the screen reach the terminal's scrollback whole.
//!
//! The label row and the box follow the conversation's end, as a shell's prompt follows the output,
//! down to the bottom of the terminal, where they stay once the conversation fills the rows above.
//! The box has a fixed number of rows and scrolls its text to keep the cursor in view, so it never
//! grows over the conversation. The conversation is only ever appended to.

use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::{self, ExitStatus};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use crossterm::cursor::{self, Hide, MoveTo, SetCursorStyle, Show};
use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyEvent, KeyEventKind,
};
use crossterm::terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen};
use unicode_width::UnicodeWidthStr;

use crate::context::COLD_COLOR;
use crate::keys::{self, Action, Phase, SessionKey};
use crate::limits::{Level, limit_level};
use crate::render::{Output, STATUS_COLOR, Status, answer_label, marks, start_bar_row};
use crate::status;
use crate::terminal::{Settings, run_in_terminal};
use crate::vi::{Editor, Effect, Mode};
use crate::wrap;
use crate::{Shared, lock};

const RESET: &str = "\x1b[0m";

/// The rows of the input box.
// TODO: Let the user choose the input box's height
const INPUT_ROWS: u16 = 3;

/// The rows the conversation keeps at least, since tmux ignores a scroll region of one row.
const MIN_CONVERSATION_ROWS: u16 = 2;

/// How long the input waits for a key before it looks again for output and a changed label.
const TICK: Duration = Duration::from_millis(25);

// The terminal shows a frame between the two at once
const BEGIN_FRAME: &str = "\x1b[?2026h";
const END_FRAME: &str = "\x1b[?2026l";

/// Writes crossterm's commands to `out`, a frame in memory, which writing cannot fail.
macro_rules! put {
    ($out:expr, $($command:expr),+ $(,)?) => {
        $({
            let mut text = String::new();
            // Formatting into a String never fails
            let _ = crossterm::Command::write_ansi(&$command, &mut text);
            $out.extend(text.as_bytes());
        })+
    };
}

/// How reading the input line ended.
pub enum Outcome {
    /// The user sends the text on the line.
    Send(String),
    /// The user stops Claude's turn, with the text on the line, which stays.
    Stop(String),
    /// The user ends the session.
    End,
    /// Claude Code asked something, which the main thread acts on first, with the text on the
    /// line.
    Break(String),
}

/// The bottom of the terminal, and where the conversation above it ends.
pub struct InputLine {
    shared: Arc<Mutex<Shared>>,
    break_signal: Arc<AtomicBool>,
    editor_command: String,
    /// The file of the full view, which the output writes to.
    full_view: PathBuf,
    /// The terminal's settings from before the client changed them, for the programs the
    /// client runs in it.
    original_settings: Settings,
    editor: Editor,
    layout: Layout,
    /// The first row of the text the box shows.
    scroll: u16,
    end: End,
    /// Lines that wait for a screen with the rows to scroll them.
    held: Vec<String>,
    /// What the last draw left above the cursor, which a resize moves before the client hears
    /// of it.
    drawn: Drawn,
    /// The label last drawn, which the input line redraws when it changes.
    label: String,
    is_closed: bool,
}

impl InputLine {
    /// The label row and the box, from the row the cursor is on, where the conversation starts.
    /// A screen with too few rows below the cursor scrolls up to make them.
    pub fn new(
        editor_command: String,
        full_view: PathBuf,
        original_settings: Settings,
        break_signal: Arc<AtomicBool>,
        shared: Arc<Mutex<Shared>>,
    ) -> Result<Self, String> {
        let (width, height) = terminal::size().map_err(|error| cannot_read_terminal(&error))?;
        let (_, row) = cursor::position().map_err(|error| cannot_read_terminal(&error))?;
        let layout = Layout::new(width, height);
        let mut out = Vec::new();
        let lowest = layout.lowest_label_row();
        if row > lowest {
            scroll_screen(&mut out, layout, row - lowest);
        }
        put!(out, EnableBracketedPaste);
        write_out(&out)?;
        Ok(InputLine {
            shared,
            break_signal,
            editor_command,
            full_view,
            original_settings,
            editor: Editor::new(""),
            layout,
            scroll: 0,
            end: End::Before(row.min(lowest)),
            held: Vec::new(),
            drawn: Drawn::default(),
            label: String::new(),
            is_closed: false,
        })
    }

    /// Reads the input line, starting from `draft`, while the conversation goes on above it.
    pub fn read(&mut self, phase: Phase, draft: &str) -> Result<Outcome, String> {
        lock(&self.shared).phase = phase;
        // A command typed for another line has nothing left to act on
        self.editor.cancel();
        if self.text() != draft {
            self.editor.replace(draft);
        }
        self.draw()?;
        loop {
            // Taking the signal lets a line opened again for the same question wait for keys
            if self.break_signal.swap(false, Ordering::Relaxed) {
                return Ok(Outcome::Break(self.text()));
            }
            self.print_output()?;
            if self.make_label() != self.label {
                self.draw()?;
            }
            if !event::poll(TICK).map_err(|error| cannot_read_terminal(&error))? {
                continue;
            }
            match event::read().map_err(|error| cannot_read_terminal(&error))? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if let Some(outcome) = self.key(key)? {
                        return Ok(outcome);
                    }
                }
                Event::Paste(text) if lock(&self.shared).phase.takes_text() => self.paste(&text),
                Event::Resize(width, height) => self.resize(width, height)?,
                _ => continue,
            }
            self.draw()?;
        }
    }

    /// Takes the bottom of the terminal away, once the session ended, and leaves the cursor at
    /// the start of the row below the conversation, where the end of the output follows.
    pub fn close(&mut self) -> Result<(), String> {
        if self.is_closed {
            return Ok(());
        }
        self.is_closed = true;
        let mut out = Vec::new();
        put!(
            out,
            MoveTo(0, self.label_row()),
            Clear(ClearType::FromCursorDown),
            SetCursorStyle::DefaultUserShape,
            DisableBracketedPaste,
        );
        put!(out, MoveTo(0, self.end.row()));
        if let End::On(_) = self.end {
            out.extend(b"\r\n");
        }
        for line in std::mem::take(&mut self.held) {
            out.extend(format!("{line}\r\n").as_bytes());
        }
        put!(out, Show);
        write_out(&out)
    }

    fn text(&self) -> String {
        self.editor.text()
    }

    /// The row of the label: right below the conversation's end, down to the lowest row the
    /// label row and the box fit above.
    fn label_row(&self) -> u16 {
        self.end.next_row().min(self.layout.lowest_label_row())
    }

    /// Whether the input line takes what the user types, which the ending has no one to read.
    /// Acts on a key, with an outcome for one that ends the reading.
    fn key(&mut self, key: KeyEvent) -> Result<Option<Outcome>, String> {
        let phase = lock(&self.shared).phase.clone();
        let action = keys::action(
            &phase,
            &key,
            self.editor.is_empty(),
            self.editor.awaits_character(),
        );
        // A key the session acts on ends the command the vi keys were in the middle of
        if action != Action::Edit {
            self.editor.cancel();
        }
        match action {
            Action::Session(SessionKey::Stop) => return Ok(Some(Outcome::Stop(self.text()))),
            Action::Session(SessionKey::End) => return Ok(Some(Outcome::End)),
            Action::Session(SessionKey::ShowFullView) => self.show_full_view()?,
            Action::Session(SessionKey::ShowStatus) => self.show_status()?,
            Action::Send => return Ok(Some(Outcome::Send(self.text()))),
            Action::Edit => {
                if self.editor.press(&key) == Effect::OpenEditor {
                    self.edit_in_editor()?;
                }
            }
            Action::Ignore => {}
        }
        Ok(None)
    }

    fn paste(&mut self, text: &str) {
        self.editor.paste(text);
    }

    /// Opens the text in the user's editor, and takes back what it saved, to add to or send.
    fn edit_in_editor(&mut self) -> Result<(), String> {
        let path = env::temp_dir().join(format!("claudio-{}.md", process::id()));
        fs::write(&path, self.text())
            .map_err(|error| format!("Cannot write {}: {error}", path.display()))?;
        let status = self
            .run_program(process::Command::new(&self.editor_command).arg(&path))?
            .map_err(|error| format!("Cannot run {}: {error}", self.editor_command))?;
        let text = fs::read_to_string(&path)
            .map_err(|error| format!("Cannot read {}: {error}", path.display()))?;
        // The file goes with the draft, which the input line now holds
        let _ = fs::remove_file(&path);
        if status.success() {
            // Editors end a file with a line break, which the message does not have
            let text = text.strip_suffix('\n').unwrap_or(&text);
            self.editor.replace(text);
        }
        Ok(())
    }

    /// Shows the status of the session as it stands, at the bottom of the terminal's alternate
    /// screen, until the next key, which only closes it, or until Claude Code asks something.
    /// The conversation stays as it was on the main screen, and its new lines wait for the
    /// status to close. A resize meanwhile moves the status to the new bottom and reaches the
    /// input box once the status closes. The main screen comes back whatever ends the status.
    fn show_status(&mut self) -> Result<(), String> {
        let lines = {
            let shared = lock(&self.shared);
            status::lines(
                shared.model.as_deref(),
                shared.session_id.as_deref(),
                &shared.limits,
                &shared.context,
                Instant::now(),
                SystemTime::now(),
            )
        };
        let mut out = Vec::new();
        put!(out, EnterAlternateScreen, Hide);
        write_out(&out)?;
        let shown = self.show_status_until_closed(&lines);
        let mut out = Vec::new();
        put!(out, LeaveAlternateScreen);
        let left = write_out(&out);
        match shown? {
            Some((width, height)) => left.and_then(|()| self.resize(width, height)),
            None => left,
        }
    }

    /// Draws the status's `lines` on the alternate screen until a key or a question from Claude
    /// Code closes it. Returns the size of the last resize meanwhile, if any.
    fn show_status_until_closed(&self, lines: &[String]) -> Result<Option<(u16, u16)>, String> {
        let mut height = self.layout.height;
        let mut resized = None;
        let mut is_drawn = false;
        while !self.break_signal.load(Ordering::Relaxed) {
            if !is_drawn {
                let top = height.saturating_sub(u16::try_from(lines.len()).unwrap_or(u16::MAX));
                let mut out = Vec::new();
                put!(out, Clear(ClearType::All), MoveTo(0, top));
                out.extend(lines.join("\r\n").as_bytes());
                write_out(&out)?;
                is_drawn = true;
            }
            if !event::poll(TICK).map_err(|error| cannot_read_terminal(&error))? {
                continue;
            }
            match event::read().map_err(|error| cannot_read_terminal(&error))? {
                Event::Key(key) if key.kind == KeyEventKind::Press => break,
                Event::Resize(new_width, new_height) => {
                    height = new_height;
                    resized = Some((new_width, new_height));
                    is_drawn = false;
                }
                _ => {}
            }
        }
        Ok(resized)
    }

    /// Shows the full view in less, from its end, where `F` follows the lines that keep coming
    /// until Ctrl-C. less keeps the colors, draws on a screen of its own, which leaves the
    /// conversation's rows as they were, and stays open on a view shorter than a screen,
    /// whatever `LESS` says. It reads the file as the UTF-8 the output writes, whatever the
    /// locale. Each of these takes a flag of less's own, which another pager has no use for, so
    /// the view runs less whatever `PAGER` says.
    fn show_full_view(&mut self) -> Result<(), String> {
        let status = self.run_program(
            process::Command::new("less")
                .args(["-R", "-+X", "-+F", "+G"])
                .env("LESSCHARSET", "utf-8")
                .arg(&self.full_view),
        )?;
        if let Err(error) = status {
            lock(&self.shared)
                .output
                .show_error(&format!("Cannot show the full view: {error}"));
        }
        Ok(())
    }

    /// Runs `command` in the terminal, handed over with paste mode off and the cursor in the
    /// user's own shape. Paste mode comes back on once the command ends. Writing to the terminal
    /// fails the outer result, and running the command the inner one.
    fn run_program(
        &self,
        command: &mut process::Command,
    ) -> Result<io::Result<ExitStatus>, String> {
        let mut out = Vec::new();
        put!(out, DisableBracketedPaste, SetCursorStyle::DefaultUserShape);
        write_out(&out)?;
        let status = run_in_terminal(self.original_settings, command);
        let mut out = Vec::new();
        put!(out, EnableBracketedPaste);
        write_out(&out)?;
        Ok(status)
    }

    /// Prints the conversation's new lines above the label row, in a scroll region over the
    /// conversation's rows, so the lines that leave the screen go to the scrollback.
    fn print_output(&mut self) -> Result<(), String> {
        let lines = lock(&self.shared).output.take_lines();
        self.held.extend(lines);
        if self.held.is_empty() || !self.layout.shows_conversation() {
            return Ok(());
        }
        let mut out = Vec::new();
        out.extend(BEGIN_FRAME.as_bytes());
        // The lines can take the rows of the label and the box, which follow them once drawn again
        put!(
            out,
            Hide,
            MoveTo(0, self.label_row()),
            Clear(ClearType::FromCursorDown)
        );
        // Setting the region puts the cursor home, so the moves follow it
        let conversation_rows = self.layout.conversation_rows();
        out.extend(format!("\x1b[1;{conversation_rows}r").as_bytes());
        let (row, mut is_first) = match self.end {
            End::Before(row) if row < conversation_rows => (row, true),
            // The label was at the bottom, below the region, so the first line scrolls the
            // region's rows up and starts its last row
            End::Before(_) => (conversation_rows - 1, false),
            End::On(row) => (row, false),
        };
        put!(out, MoveTo(0, row));
        for line in std::mem::take(&mut self.held) {
            for part in line.split('\n') {
                if !is_first {
                    out.extend(b"\r\n");
                }
                is_first = false;
                out.extend(part.as_bytes());
            }
        }
        write_out(&out)?;
        // The cursor is where the last line ends: past its last character, or on it when the
        // line ends at the row's end
        let (_, row) = cursor::position().map_err(|error| cannot_read_terminal(&error))?;
        self.end = End::On(row);
        // The frame ends with the label and the box drawn below the lines
        write_out(b"\x1b[r")?;
        self.draw()
    }

    /// Draws the label row and the input box, with the cursor in the box.
    fn draw(&mut self) -> Result<(), String> {
        self.label = self.make_label();
        let layout = self.layout;
        let mut out = Vec::new();
        out.extend(BEGIN_FRAME.as_bytes());
        put!(out, Hide);
        let label_row = self.label_row();
        let mut widths = Vec::new();
        if layout.label_rows > 0 {
            put!(out, MoveTo(0, label_row));
            out.extend(format!("{}{}{RESET}", start_bar_row(), self.label).as_bytes());
            widths.push(visible_width(&self.label));
        }
        let wrapped = wrap::wrap(
            &self.text(),
            self.editor.cursor(),
            self.editor.selection(),
            usize::from(layout.width),
        );
        let to_row = |index: usize| u16::try_from(index).unwrap_or(u16::MAX);
        let (cursor_x, cursor_y) = (to_row(wrapped.cursor_column), to_row(wrapped.cursor_row));
        let used_rows = to_row(wrapped.rows.len());
        // The box scrolls as little as keeps the cursor in it, and shows no empty rows below the
        // text that the text above could fill
        let rows = layout.input_rows;
        self.scroll = self
            .scroll
            .min(used_rows.max(cursor_y + 1).saturating_sub(rows))
            .min(cursor_y)
            .max((cursor_y + 1).saturating_sub(rows));
        let mut row_widths = Vec::new();
        for y in 0..rows {
            let row = label_row + layout.label_rows + y;
            if row >= layout.height {
                break;
            }
            put!(out, MoveTo(0, row));
            let (text, width) = wrapped
                .rows
                .get(usize::from(self.scroll + y))
                .map_or(("", 0), |row| (row.text.as_str(), row.width));
            out.extend(format!("{}{text}{RESET}", start_bar_row()).as_bytes());
            row_widths.push(width);
        }
        let cursor_in_box = cursor_y - self.scroll;
        let cursor_row =
            (label_row + layout.label_rows + cursor_in_box).min(layout.height.saturating_sub(1));
        widths.extend(row_widths.iter().take(usize::from(cursor_in_box)));
        self.drawn = Drawn {
            widths_above_cursor: widths,
            cursor_column: cursor_x,
        };
        let shape = match self.editor.mode() {
            Mode::Insert => SetCursorStyle::SteadyBar,
            Mode::Normal | Mode::Visual | Mode::VisualLine => SetCursorStyle::SteadyBlock,
        };
        put!(out, MoveTo(cursor_x, cursor_row), shape, Show);
        out.extend(END_FRAME.as_bytes());
        write_out(&out)
    }

    /// Puts the bottom of the terminal back in place after a resize. The terminal moved it
    /// with the cursor before the client heard of the resize: a narrower screen splits each of
    /// its rows wider than the screen, and a taller one can leave it in the middle.
    ///
    /// A screen shrunk to no more rows than the label row and the box's rows above the cursor
    /// takes those rows into the scrollback, where copies of them stay.
    fn resize(&mut self, width: u16, height: u16) -> Result<(), String> {
        let (_, cursor_row) = cursor::position().map_err(|error| cannot_read_terminal(&error))?;
        let width_now = usize::from(width.max(1));
        let rows_above: usize = self
            .drawn
            .widths_above_cursor
            .iter()
            .map(|width| width.div_ceil(width_now).max(1))
            .sum::<usize>()
            + usize::from(self.drawn.cursor_column) / width_now;
        let label_row = cursor_row.saturating_sub(u16::try_from(rows_above).unwrap_or(u16::MAX));
        let mut out = Vec::new();
        put!(out, MoveTo(0, label_row), Clear(ClearType::FromCursorDown));
        self.layout = Layout::new(width, height);
        // The label row is right below the conversation's end
        self.end = match self.end {
            End::Before(_) => End::Before(label_row),
            End::On(_) => label_row.checked_sub(1).map_or(End::Before(0), End::On),
        };
        // A shorter screen has the conversation reach into the rows the label row and the box
        // take, so it scrolls up
        let lowest = self.layout.lowest_label_row();
        if self.end.next_row() > lowest {
            let rows = self.end.next_row() - lowest;
            scroll_screen(&mut out, self.layout, rows);
            self.end = match self.end {
                End::Before(row) => End::Before(row - rows),
                End::On(row) => End::On(row - rows),
            };
        }
        write_out(&out)
    }

    /// The label row: for a message, `››` with the context's level, marked and red on a cold
    /// cache, the levels of the plan's usage limits, and what Claude does after them; for an
    /// answer, the question; at the end, the work Claude still does.
    fn make_label(&self) -> String {
        let shared = lock(&self.shared);
        match &shared.phase {
            Phase::Writing => {
                let now = Instant::now();
                let level = shared.context.level();
                let is_cold = shared.context.is_cold(now);
                // On a cold cache, the space in front of T becomes *, which shows the cold cache
                // without color and keeps the label's width
                let (cold_mark, level_color) = if is_cold {
                    ("*", COLD_COLOR)
                } else {
                    (" ", STATUS_COLOR)
                };
                let indicators = format!(
                    "{}{level_color}{cold_mark}T{} {} {}",
                    marks(),
                    show_level(level),
                    limit_indicator('S', shared.limits.session().map(limit_level)),
                    limit_indicator('W', shared.limits.week().map(limit_level)),
                );
                let state = if shared.context.is_cold_confirmed(now) {
                    Some("cold, enter sends".to_string())
                } else {
                    describe_work(&shared.output)
                };
                let state =
                    state.map_or_else(String::new, |state| format!(" {STATUS_COLOR}│ {state}"));
                format!("{indicators}{state}")
            }
            Phase::Answering(question) => answer_label(question),
            Phase::Ending => {
                let output = &shared.output;
                let tasks = output.background_tasks();
                let background =
                    (!tasks.is_empty()).then(|| format!("background: {}", tasks.join(", ")));
                let parts: Vec<String> = ["ending".to_string()]
                    .into_iter()
                    .chain(describe_work(output))
                    .chain(background)
                    .collect();
                format!("{} {STATUS_COLOR}{}", marks(), parts.join(" │ "))
            }
        }
    }
}

impl Drop for InputLine {
    /// Gives the terminal back as it was, for a session that ended without closing the input
    /// line, on an error.
    fn drop(&mut self) {
        // The terminal is being left, with nowhere left to tell a failure
        let _ = self.close();
    }
}

/// Where the parts of the screen are, for a terminal of `width` and `height`.
#[derive(Clone, Copy)]
struct Layout {
    width: u16,
    height: u16,
    /// One for the label row, none on a screen of one row, which only the row typed in fits.
    label_rows: u16,
    input_rows: u16,
}

impl Layout {
    fn new(width: u16, height: u16) -> Self {
        // A short screen keeps a row to type in first, then the label row, then the
        // conversation's rows, and takes the rest from the box
        let label_rows = u16::from(height >= 2);
        let input_rows = INPUT_ROWS
            .min(height.saturating_sub(MIN_CONVERSATION_ROWS + label_rows))
            .max(1);
        Layout {
            width: width.max(1),
            height,
            label_rows,
            input_rows,
        }
    }

    /// The rows above the lowest label row, where the conversation scrolls.
    fn conversation_rows(self) -> u16 {
        self.height
            .saturating_sub(self.input_rows + self.label_rows)
    }

    /// The row of the label once the conversation reaches the bottom of the terminal, with the
    /// box below it.
    fn lowest_label_row(self) -> u16 {
        self.conversation_rows()
    }

    /// Whether the conversation has the rows to scroll in. On a shorter screen its lines wait.
    fn shows_conversation(self) -> bool {
        self.conversation_rows() >= MIN_CONVERSATION_ROWS
    }
}

/// Where the conversation ends on screen, which its next line follows.
#[derive(Clone, Copy)]
enum End {
    /// Nothing printed yet: the first line goes on this row, an empty one.
    Before(u16),
    /// The last line printed ends on this row.
    On(u16),
}

impl End {
    fn row(self) -> u16 {
        match self {
            End::Before(row) | End::On(row) => row,
        }
    }

    /// The first row below the conversation.
    fn next_row(self) -> u16 {
        match self {
            End::Before(row) => row,
            End::On(row) => row + 1,
        }
    }
}

/// What the last draw left above the cursor.
#[derive(Default)]
struct Drawn {
    /// The widths of the text on the label row and on the box's rows above the cursor, which
    /// a narrower screen splits into the rows that text then takes.
    widths_above_cursor: Vec<usize>,
    cursor_column: u16,
}

/// The columns `text` takes on screen, without its escape sequences.
fn visible_width(text: &str) -> usize {
    let mut visible = String::new();
    let mut characters = text.chars();
    while let Some(character) = characters.next() {
        if character == '\x1b' {
            // Every sequence the label holds is a CSI, which ends with a letter
            for next in characters.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            visible.push(character);
        }
    }
    visible.width()
}

/// Scrolls the whole screen up by `rows`, which sends its top rows to the scrollback.
fn scroll_screen(out: &mut Vec<u8>, layout: Layout, rows: u16) {
    put!(out, MoveTo(0, layout.height.saturating_sub(1)));
    for _ in 0..rows {
        out.extend(b"\n");
    }
}

fn write_out(out: &[u8]) -> Result<(), String> {
    let mut stdout = io::stdout();
    stdout
        .write_all(out)
        .and_then(|()| stdout.flush())
        .map_err(|error| format!("Cannot write to the terminal: {error}"))
}

fn cannot_read_terminal(error: &io::Error) -> String {
    format!("Cannot read the terminal: {error}")
}

/// The last line of the session, on the bar, with the session's id once Claude Code told it.
pub fn final_line(session_id: Option<&str>) -> String {
    let session = session_id.map_or_else(String::new, |id| format!(" │ session {id}"));
    format!(
        "{}{} {STATUS_COLOR}ended{session}{RESET}",
        start_bar_row(),
        marks()
    )
}

/// A level, with ? for one not known yet.
fn show_level(level: Option<usize>) -> String {
    level.map_or_else(|| "?".to_string(), |level| level.to_string())
}

/// The letter of a usage limit and its level, in the level's color, so the color only repeats
/// what the level says.
fn limit_indicator(letter: char, level: Option<Level>) -> String {
    let color = level.and_then(Level::color).unwrap_or(STATUS_COLOR);
    format!("{color}{letter}{}", show_level(level.map(Level::to_number)))
}

/// What Claude does, none while it waits for the user.
fn describe_work(output: &Output) -> Option<String> {
    match output.status() {
        Status::Idle => None,
        Status::Working => Some("working".to_string()),
        Status::Writing if output.has_waiting_stderr() => {
            Some("writing, stderr waiting".to_string())
        }
        Status::Writing => Some("writing".to_string()),
        Status::Running(tool) => Some(format!("running {tool}")),
    }
}
