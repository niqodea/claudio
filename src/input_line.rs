//! The bottom of the terminal: the label row, which shows the context, the usage limits and what
//! Claude does, and below it the input box, where the user types with vi keys. The conversation
//! scrolls above them, so the lines that leave the screen reach the terminal's scrollback whole.
//!
//! The label row and the box follow the conversation's end, as a shell's prompt follows the output,
//! down to the bottom of the terminal, where they stay once the conversation fills the rows above.
//! The box has a fixed number of rows and scrolls its text to keep the cursor in view, so it never
//! grows over the conversation.
//!
//! The conversation is only ever appended to, a whole line at a time. The line Claude is still
//! writing shows above the label row until it ends, and together they are the live rows, which
//! each frame erases and draws again, after climbing back to the first of them as `climb` tells.

use std::env;
use std::fs;
use std::io::{self, Write};
use std::num::{NonZeroU16, NonZeroUsize};
use std::path::PathBuf;
use std::process::{self, ExitStatus};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use crossterm::cursor::{self, Hide, MoveRight, MoveTo, SetCursorStyle, Show};
use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers,
};
use crossterm::terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::climb::{self, Climb};
use crate::context::COLD_COLOR;
use crate::keys::{self, Action, Phase, SessionKey, View};
use crate::limits::{Level, limit_level};
use crate::picker::{self, Cursor, Draft, Form, Move, Pick, question_name};
use crate::render::{Output, Status, colors_at, cut, marks, start_bar_row, user_line};
use crate::status;
use crate::style::{BOLD, GREEN, ORANGE, RESET, REVERSE, STATUS_COLOR, UNREVERSE};
use crate::terminal::{Settings, run_in_terminal};
use crate::vi::{Editor, Mode};
use crate::wrap;
use crate::{Shared, lock};

/// The rows the conversation keeps at least, above the label row and the box.
const MIN_CONVERSATION_ROWS: u16 = 1;

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

/// How reading the input line ended, with `D`, the draft: the text on the line, or the pick of
/// the question the question view shows.
pub enum Outcome<D> {
    /// The user sends the text on the line, or picks an answer.
    Send(D),
    /// The user stops Claude's turn, with the draft.
    Stop(D),
    /// The user opens the questions that wait, or goes back from them to the message, with the
    /// draft of what the user leaves.
    Questions(D),
    /// The user moves to another question, with the draft of the question left.
    Go(D, Move),
    /// The user ends the session.
    End,
    /// The user clears the context, with the draft, which stays.
    Clear(D),
    /// The user runs one of Claude Code's commands, written without its `/`, with the draft,
    /// which stays.
    Run(D, String),
    /// Claude Code asked something, which the main thread acts on first, with the draft.
    Break(D),
}

impl Outcome<()> {
    /// The same outcome, with `draft`.
    fn with<D>(self, draft: D) -> Outcome<D> {
        match self {
            Outcome::Send(()) => Outcome::Send(draft),
            Outcome::Stop(()) => Outcome::Stop(draft),
            Outcome::Questions(()) => Outcome::Questions(draft),
            Outcome::Go((), to) => Outcome::Go(draft, to),
            Outcome::End => Outcome::End,
            Outcome::Clear(()) => Outcome::Clear(draft),
            Outcome::Run((), command) => Outcome::Run(draft, command),
            Outcome::Break(()) => Outcome::Break(draft),
        }
    }
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
    /// The rows the user chose for the input box, which a short screen gives fewer.
    input_rows: NonZeroU16,
    editor: Editor,
    /// The field of the command of Claude Code's that the user writes after `:`, without its
    /// `/`, while the message waits in the box, none while the command line is closed. It is a
    /// field in insert mode, as the box can be, with no normal mode to leave to, so Esc closes it.
    command_line: Option<Editor>,
    /// What the user picks on the question the question view shows.
    pick: Pick,
    /// Whether the last key's answer answered nothing, so the question view held it back, which
    /// the label row says.
    is_empty_warned: bool,
    /// The key pressed once that waits for its second press in a row, which the label row says.
    confirming: Option<Confirming>,
    layout: Layout,
    /// The first row of the text the box shows.
    scroll: u16,
    /// The conversation's text not printed yet: whole lines, which wait for the main screen and
    /// for a screen with rows to show them, then the line Claude still writes, which shows among
    /// the live rows until it ends.
    pending: String,
    /// How each frame gets back to the first live row.
    climb: Climb,
    /// Where the last frame left the cursor among the live rows.
    drawn: Drawn,
    /// The label last drawn, which the input line redraws when it changes.
    label: String,
    /// Whether the question view shows. It shows on the alternate screen, which keeps it out of
    /// the scrollback, since a question matters only while it is answered, and the
    /// conversation's new lines wait for the main screen to come back.
    in_view: bool,
    is_closed: bool,
}

impl InputLine {
    /// The label row and the box, from the row the cursor is on, where the conversation starts,
    /// or the row below when the cursor is past the row's start. A screen with too few rows below
    /// that row scrolls up to make them. Then it checks how frames can climb back to the live
    /// rows, and warns when they can only count their way back.
    pub fn new(
        editor_command: String,
        full_view: PathBuf,
        original_settings: Settings,
        input_rows: NonZeroU16,
        break_signal: Arc<AtomicBool>,
        shared: Arc<Mutex<Shared>>,
    ) -> Result<Self, String> {
        let (width, height) = terminal::size().map_err(|error| cannot_read_terminal(&error))?;
        let (column, row) = cursor::position().map_err(|error| cannot_read_terminal(&error))?;
        let row = if column > 0 {
            write_out(b"\r\n")?;
            cursor::position()
                .map_err(|error| cannot_read_terminal(&error))?
                .1
        } else {
            row
        };
        let layout = Layout::new(width, height, input_rows);
        let mut out = Vec::new();
        let lowest = layout.lowest_label_row();
        if row > lowest {
            scroll_screen(&mut out, layout, row - lowest);
        }
        put!(out, MoveTo(0, row.min(lowest)), EnableBracketedPaste);
        write_out(&out)?;
        // The check needs a row below the cursor, which a screen of one row lacks
        let climb = if height >= 2 {
            climb::check()?
        } else {
            Climb::Counted
        };
        if climb == Climb::Counted {
            lock(&shared).output.show_inexact_redraw();
        }
        Ok(InputLine {
            shared,
            break_signal,
            editor_command,
            full_view,
            original_settings,
            input_rows,
            editor: Editor::new(""),
            command_line: None,
            pick: Draft::new().pick,
            is_empty_warned: false,
            confirming: None,
            layout,
            scroll: 0,
            pending: String::new(),
            climb,
            // No live rows yet: the cursor is where the first one goes
            drawn: Drawn {
                lines_above_cursor: 0,
                width: layout.width,
            },
            label: String::new(),
            in_view: false,
            is_closed: false,
        })
    }

    /// Reads the input line, starting from `draft`, while the conversation goes on above it.
    pub fn read(&mut self, phase: Phase, draft: &str) -> Result<Outcome<String>, String> {
        // A command typed for another line has nothing left to act on
        self.editor.cancel();
        // The box held an answer in the question view, whose mode and cursor are the answer's,
        // and else keeps the message as the user left it
        if self.in_view || self.text() != draft {
            self.editor.replace(draft);
        }
        let outcome = self.run(phase)?;
        self.command_line = None;
        Ok(outcome.with(self.text()))
    }

    /// Reads what the user picks and writes on the question that `view` shows, starting from
    /// `draft`.
    pub fn read_question(&mut self, view: View, draft: Draft) -> Result<Outcome<Draft>, String> {
        self.editor.cancel();
        self.editor.replace(&draft.text);
        // The box waits in normal mode while the cursor is on the list
        if draft.pick.cursor != Cursor::Writing {
            self.editor.press(&escape());
        }
        self.pick = draft.pick;
        let outcome = self.run(Phase::Answering(view))?;
        Ok(outcome.with(Draft {
            pick: self.pick.clone(),
            text: self.text(),
        }))
    }

    /// Reads keys in `phase` until one ends the reading.
    fn run(&mut self, phase: Phase) -> Result<Outcome<()>, String> {
        let answers = matches!(phase, Phase::Answering(_));
        lock(&self.shared).phase = phase;
        // What a waiting key loses can change between readings, as with a question that arrives
        self.confirming = None;
        if answers != self.in_view {
            self.switch_screen()?;
        }
        self.draw()?;
        loop {
            // Taking the signal lets a line opened again for the same question wait for keys
            if self.break_signal.swap(false, Ordering::Relaxed) {
                return Ok(Outcome::Break(()));
            }
            self.print_output()?;
            self.drop_lost_confirmation();
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
                Event::Resize(width, height) => self.resize(width, height),
                _ => continue,
            }
            self.draw()?;
        }
    }

    /// Takes the bottom of the terminal away, once the session ended, and prints the rest of the
    /// conversation. It leaves the cursor where the conversation goes on: at the end of the line
    /// Claude was writing, if any, and else at the start of the row below the last line.
    pub fn close(&mut self) -> Result<(), String> {
        if self.is_closed {
            return Ok(());
        }
        self.is_closed = true;
        if self.in_view {
            self.switch_screen()?;
        }
        let mut out = Vec::new();
        climb::write(
            &mut out,
            self.climb,
            self.drawn.lines_above_cursor,
            self.drawn.width,
        );
        out.extend(climb::ERASE_DOWN.as_bytes());
        put!(out, SetCursorStyle::DefaultUserShape, DisableBracketedPaste);
        let pending = std::mem::take(&mut self.pending);
        out.extend(pending.replace('\n', "\r\n").as_bytes());
        if self.climb == Climb::Exact {
            out.extend(climb::BACKSPACE_WRAPS_OFF.as_bytes());
        }
        put!(out, Show);
        write_out(&out)
    }

    fn text(&self) -> String {
        self.editor.text()
    }

    /// Goes to the question view's screen, or back from it to the conversation's, which comes
    /// back as it was, with the cursor where it was, and draws itself again for any resize
    /// meanwhile.
    fn switch_screen(&mut self) -> Result<(), String> {
        let mut out = Vec::new();
        if self.in_view {
            put!(out, LeaveAlternateScreen);
        } else {
            put!(out, EnterAlternateScreen);
        }
        write_out(&out)?;
        self.in_view = !self.in_view;
        Ok(())
    }

    /// Acts on a key, with an outcome for one that ends the reading.
    fn key(&mut self, key: KeyEvent) -> Result<Option<Outcome<()>>, String> {
        let phase = lock(&self.shared).phase.clone();
        self.is_empty_warned = false;
        let confirming = self.confirming.take();
        let action = keys::action(&phase, &key, self.input());
        // Any key other than an edit cancels the half-typed vi command
        if action != Action::Edit {
            self.editor.cancel();
        }
        let loss = keys::loss(&phase, action, self.stakes());
        if !self.goes_ahead(&key, confirming.as_ref(), loss, Some(action)) {
            return Ok(None);
        }
        // A session key acts on the session, and closes the command line
        if matches!(action, Action::Session(_)) {
            self.command_line = None;
        }
        match action {
            Action::Session(SessionKey::Stop) => return Ok(Some(Outcome::Stop(()))),
            Action::Session(SessionKey::End) => return Ok(Some(Outcome::End)),
            Action::Session(SessionKey::Clear) => return Ok(Some(Outcome::Clear(()))),
            Action::CommandLine => self.command_line = Some(Editor::new("")),
            Action::Run => {
                let command = self.command_line.take().map(|line| line.text());
                let command = command.as_deref().unwrap_or_default().trim();
                return Ok((!command.is_empty()).then(|| Outcome::Run((), command.to_string())));
            }
            Action::Session(SessionKey::Questions) => return Ok(Some(Outcome::Questions(()))),
            Action::Session(direction @ (SessionKey::Down | SessionKey::Up)) => {
                if let Phase::Answering(view) = &phase
                    && !view.is_submitted
                {
                    let choices = view.choices.len();
                    let effect = if direction == SessionKey::Down {
                        self.pick.move_down(choices, view.form)
                    } else {
                        self.pick.move_up(choices)
                    };
                    return Ok(self.carry_out(effect, &key));
                }
            }
            Action::Session(SessionKey::ShowFullView) => self.show_full_view()?,
            Action::Session(SessionKey::ShowStatus) => self.show_status()?,
            // The question view's text is the user's own answer, which the box holds only while
            // the cursor is on it
            Action::Session(SessionKey::Edit) => match &phase {
                Phase::Answering(view)
                    if view.is_submitted || self.pick.cursor != Cursor::Writing => {}
                _ => self.edit_in_editor()?,
            },
            // The question view answers to Enter and the other keys on its own
            Action::Send | Action::Edit => {
                if let Phase::Answering(view) = &phase {
                    return Ok(self.answer_key(view, &key, confirming.as_ref()));
                }
                if action == Action::Send {
                    return Ok(Some(Outcome::Send(())));
                }
                match &mut self.command_line {
                    Some(_) if key.code == KeyCode::Esc => self.command_line = None,
                    Some(line) => line.press(&key),
                    None => self.editor.press(&key),
                }
            }
            Action::Ignore => {}
        }
        Ok(None)
    }

    /// What the keys depend on in the input line.
    fn input(&self) -> keys::Input {
        keys::Input {
            is_idle: self.editor.mode() == Mode::Normal && self.editor.pending().is_empty(),
            is_empty: self.text().trim().is_empty(),
            is_commanding: self.command_line.is_some(),
        }
    }

    /// Whether `key` goes ahead: at once when it loses nothing, and else on its second press in
    /// a row, which `confirming` holds the first of, with the same loss. A loss that changed
    /// between the presses waits for a press more, so the user always saw what a key loses
    /// before it goes ahead. Meanwhile the label row names the loss.
    fn goes_ahead(
        &mut self,
        key: &KeyEvent,
        confirming: Option<&Confirming>,
        loss: Option<String>,
        action: Option<Action>,
    ) -> bool {
        let Some(loss) = loss else {
            return true;
        };
        let is_second = confirming.is_some_and(|first| {
            first.key.code == key.code && first.key.modifiers == key.modifiers && first.loss == loss
        });
        if !is_second {
            self.confirming = Some(Confirming {
                key: *key,
                loss,
                action,
            });
        }
        is_second
    }

    /// What the session holds that a press can lose.
    fn stakes(&self) -> keys::Stakes {
        let shared = lock(&self.shared);
        let text = self
            .command_line
            .as_ref()
            .map_or_else(|| self.text(), Editor::text);
        keys::Stakes {
            has_text: !text.trim().is_empty(),
            held: shared.context.held(Instant::now()),
            is_working: shared.output.status() != Status::Idle,
        }
    }

    /// Ends the wait of a key whose loss went away, so the label row never asks for a press
    /// that loses nothing. A loss that changed instead waits for the press, which names it.
    fn drop_lost_confirmation(&mut self) {
        let Some(action) = self.confirming.as_ref().and_then(|first| first.action) else {
            return;
        };
        let phase = lock(&self.shared).phase.clone();
        if keys::loss(&phase, action, self.stakes()).is_none() {
            self.confirming = None;
        }
    }

    /// Acts on a key of the question view's that is not a session key, Enter among them. An
    /// answer that leaves out the user's own text waits for the same key again, after
    /// `confirming`, the key before.
    fn answer_key(
        &mut self,
        view: &View,
        key: &KeyEvent,
        confirming: Option<&Confirming>,
    ) -> Option<Outcome<()>> {
        if view.is_submitted {
            return self.carry_out(Pick::press_submitted(key), key);
        }
        let text = self.text();
        let before = self.pick.clone();
        let effect = self
            .pick
            .press(key, view.choices.len(), view.form, self.input());
        if effect == picker::Effect::Submit && view.form != Form::Review {
            let is_empty = self.pick.chosen(view.form, &text).is_empty();
            let loss = self
                .pick
                .leaves_out(view.form, &text)
                .then(|| "leaves out your own answer".to_string());
            if is_empty || !self.goes_ahead(key, confirming, loss, None) {
                self.pick = before;
                self.is_empty_warned = is_empty;
                return None;
            }
        }
        self.carry_out(effect, key)
    }

    /// Carries out what `key` does in the question view besides changing the pick.
    fn carry_out(&mut self, effect: picker::Effect, key: &KeyEvent) -> Option<Outcome<()>> {
        match effect {
            picker::Effect::Stay => None,
            picker::Effect::Type => {
                self.editor.press(key);
                None
            }
            picker::Effect::StartWriting => {
                self.enter_box();
                None
            }
            picker::Effect::StopWriting => {
                if self.editor.mode() != Mode::Normal {
                    self.editor.press(&escape());
                }
                None
            }
            picker::Effect::Submit => Some(Outcome::Send(())),
            picker::Effect::Back => Some(Outcome::Questions(())),
            picker::Effect::Go(to) => Some(Outcome::Go((), to)),
        }
    }

    /// Puts the box in insert mode, at the end of its text, as vi's `A` on the last line.
    fn enter_box(&mut self) {
        self.editor.replace(&self.text());
    }

    /// Pastes `text` into the box, which the question view takes the cursor into first, on a
    /// question open to answers.
    fn paste(&mut self, text: &str) {
        self.confirming = None;
        // The command line holds one line
        if let Some(line) = &mut self.command_line {
            line.paste(&text.replace(|c: char| c.is_control(), " "));
            return;
        }
        let view = match &lock(&self.shared).phase {
            Phase::Answering(view) => Some(view.clone()),
            _ => None,
        };
        if let Some(view) = &view
            && (view.is_submitted || view.form == Form::Review)
        {
            return;
        }
        if view.is_some() && self.pick.cursor != Cursor::Writing {
            self.pick.start_writing();
            self.enter_box();
        }
        self.editor.paste(text);
    }

    /// Opens the text in the user's editor, and takes back what it saved, to add to or send.
    fn edit_in_editor(&mut self) -> Result<(), String> {
        let path = env::temp_dir().join(format!("claudio-{}.md", process::id()));
        fs::write(&path, self.text())
            .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
        let status = match self
            .run_program(process::Command::new(&self.editor_command).arg(&path))?
        {
            Ok(status) => status,
            // The draft stays on the line, and the session goes on
            Err(error) => {
                let _ = fs::remove_file(&path);
                return self.show_error(&format!("cannot run {}: {error}", self.editor_command));
            }
        };
        let text = fs::read_to_string(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
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
        self.show_screen(&lines)
    }

    /// Shows `error` on a screen of its own, like a program the input line runs, since it
    /// concerns the input line rather than the conversation.
    fn show_error(&mut self, error: &str) -> Result<(), String> {
        self.show_screen(&[error.to_string()])
    }

    /// Shows `lines` at the bottom of the terminal's alternate screen, until the next key or a
    /// question from Claude Code closes them. The conversation stays as it was on the main
    /// screen, which comes back once they close.
    fn show_screen(&mut self, lines: &[String]) -> Result<(), String> {
        // The question view is on the alternate screen already, and draws itself again after
        let mut out = Vec::new();
        if !self.in_view {
            put!(out, EnterAlternateScreen);
        }
        put!(out, Hide);
        write_out(&out)?;
        let shown = self.show_screen_until_closed(lines);
        let mut out = Vec::new();
        if !self.in_view {
            put!(out, LeaveAlternateScreen);
        }
        let left = write_out(&out);
        if let Some((width, height)) = shown? {
            self.resize(width, height);
        }
        left
    }

    /// Draws `lines` on the alternate screen until a key or a question from Claude Code closes
    /// it. Returns the size of the last resize meanwhile, if any.
    fn show_screen_until_closed(&self, lines: &[String]) -> Result<Option<(u16, u16)>, String> {
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
    /// until Ctrl+C, or from the question the question view shows, which reads in full there
    /// with less's own keys and then answers on `q`. less keeps the colors, draws on a screen of its own, which leaves the
    /// conversation's rows as they were, and stays open on a view shorter than a screen,
    /// whatever `LESS` says. It reads the file as the UTF-8 the output writes, whatever the
    /// locale. Each of these takes a flag of less's own, which another pager has no use for, so
    /// the view runs less whatever `PAGER` says.
    fn show_full_view(&mut self) -> Result<(), String> {
        let start = match &lock(&self.shared).phase {
            Phase::Answering(view) => format!("+{}g", view.full_view_line),
            _ => "+G".to_string(),
        };
        let status = self.run_program(
            process::Command::new("less")
                .args(["-R", "-+X", "-+F", &start])
                .env("LESSCHARSET", "utf-8")
                .arg(&self.full_view),
        )?;
        match status {
            Ok(_) => Ok(()),
            Err(error) => self.show_error(&format!("cannot show the full view: {error}")),
        }
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
        // A program that draws on the alternate screen leaves it as it ends
        if self.in_view {
            put!(out, EnterAlternateScreen);
        }
        put!(out, EnableBracketedPaste);
        write_out(&out)?;
        Ok(status)
    }

    /// Takes the conversation's new text, and draws the frame that prints it, once the main
    /// screen shows.
    fn print_output(&mut self) -> Result<(), String> {
        let text = lock(&self.shared).output.take_text();
        if text.is_empty() {
            return Ok(());
        }
        self.pending.push_str(&text);
        if self.in_view {
            return Ok(());
        }
        self.draw()
    }

    /// Draws the screen the user is on: the question view, or the main screen.
    fn draw(&mut self) -> Result<(), String> {
        self.label = self.make_label();
        let view = match &lock(&self.shared).phase {
            Phase::Answering(view) if self.in_view => Some(view.clone()),
            _ => None,
        };
        match view {
            Some(view) => self.draw_view(&view),
            None => self.draw_main(),
        }
    }

    /// Draws a frame of the main screen. It climbs back to the first live row and erases from
    /// there down, prints the conversation's whole lines that wait, then draws the live rows,
    /// each as a line of its own, as `climb` needs: the line Claude still writes, the label row
    /// and the box. Last, it climbs from the end of the last live row to the cursor's row, and
    /// moves along it to the cursor. No step names a row by its number, so the frame lands where
    /// it should even when a resize reaches the terminal first.
    fn draw_main(&mut self) -> Result<(), String> {
        let layout = self.layout;
        let mut out = Vec::new();
        out.extend(BEGIN_FRAME.as_bytes());
        put!(out, Hide);
        climb::write(
            &mut out,
            self.climb,
            self.drawn.lines_above_cursor,
            self.drawn.width,
        );
        out.extend(climb::ERASE_DOWN.as_bytes());
        // Each whole line ends with its line break, so the live rows start on a line of their own
        let shows_conversation = layout.shows_conversation();
        if shows_conversation && let Some(end) = self.pending.rfind('\n') {
            let lines: String = self.pending.drain(..=end).collect();
            out.extend(lines.replace('\n', "\r\n").as_bytes());
        }
        let width = usize::from(layout.width);
        let label_rows: Vec<String> = if layout.label_rows > 0 {
            terminal_rows(&self.label_line(None), width)
                .iter()
                .map(|row| bar_row(row))
                .collect()
        } else {
            Vec::new()
        };
        let (box_rows, cursor_row_in_box, cursor_column_in_box) = self.box_rows();
        let open_rows = if shows_conversation {
            let room = usize::from(layout.height).saturating_sub(label_rows.len() + box_rows.len());
            open_line_rows(&self.pending, width, room)
        } else {
            Vec::new()
        };
        let to_u16 = |index: usize| u16::try_from(index).unwrap_or(u16::MAX);
        // The command line ends the label row, where its cursor then is, as on vim's
        let (cursor_line, cursor_column) = if let Some(line) = &self.command_line {
            let after: String = line.text().chars().skip(line.cursor()).collect();
            let column = visible_width(&self.label).saturating_sub(visible_width(&after));
            (open_rows.len(), to_u16(column).min(layout.width - 1))
        } else {
            (
                open_rows.len() + label_rows.len() + cursor_row_in_box,
                cursor_column_in_box,
            )
        };
        let lines: Vec<String> = [open_rows, label_rows, box_rows].concat();
        out.extend(lines.join("\r\n").as_bytes());
        let lines_below_cursor = lines.len().saturating_sub(cursor_line + 1);
        climb::write(
            &mut out,
            self.climb,
            to_u16(lines_below_cursor),
            layout.width,
        );
        if cursor_column > 0 {
            put!(out, MoveRight(cursor_column));
        }
        self.drawn = Drawn {
            lines_above_cursor: to_u16(cursor_line),
            width: layout.width,
        };
        put!(out, self.cursor_shape(), Show);
        out.extend(END_FRAME.as_bytes());
        write_out(&out)
    }

    /// Draws the question view on the alternate screen, which has no scrollback and keeps its
    /// rows as they are on a resize, so it places its parts at fixed rows: the question above the
    /// label row, the options between the label row and the box, where the options' pointer
    /// stands for the cursor while it is on them, and the box at the bottom.
    fn draw_view(&mut self, view: &View) -> Result<(), String> {
        let layout = self.layout;
        let mut out = Vec::new();
        out.extend(BEGIN_FRAME.as_bytes());
        put!(out, Hide);
        let rows = ViewRows::new(layout, view.choices.len());
        self.draw_question(&mut out, view, rows);
        if layout.label_rows > 0 {
            put_bar_line(&mut out, rows.label, &self.label_line(Some(view)));
        }
        let (box_rows, cursor_row_in_box, cursor_column) = self.box_rows();
        for (row, line) in (rows.box_top..layout.height).zip(&box_rows) {
            put!(out, MoveTo(0, row));
            out.extend(line.as_bytes());
        }
        // The cursor shows only while the user writes in the box
        if self.pick.cursor == Cursor::Writing && !view.is_submitted {
            let row = (rows.box_top + u16::try_from(cursor_row_in_box).unwrap_or(u16::MAX))
                .min(layout.height.saturating_sub(1));
            put!(out, MoveTo(cursor_column, row), self.cursor_shape(), Show);
        }
        out.extend(END_FRAME.as_bytes());
        write_out(&out)
    }

    /// The box's rows, on the bar, with the row and the column of the cursor among them. The box
    /// scrolls as little as keeps the cursor in it, and shows no empty rows below the text that
    /// the text above could fill.
    fn box_rows(&mut self) -> (Vec<String>, usize, u16) {
        let wrapped = wrap::wrap(
            &self.text(),
            self.editor.cursor(),
            self.editor.selection(),
            usize::from(self.layout.width),
        );
        let to_u16 = |index: usize| u16::try_from(index).unwrap_or(u16::MAX);
        let cursor_y = to_u16(wrapped.cursor_row);
        let used_rows = to_u16(wrapped.rows.len());
        let rows = self.layout.input_rows;
        self.scroll = self
            .scroll
            .min(used_rows.max(cursor_y + 1).saturating_sub(rows))
            .min(cursor_y)
            .max((cursor_y + 1).saturating_sub(rows));
        let lines = (0..rows)
            .map(|y| {
                let text = wrapped
                    .rows
                    .get(usize::from(self.scroll + y))
                    .map_or("", |row| row.text.as_str());
                format!("{}{text}{RESET}", start_bar_row())
            })
            .collect();
        (
            lines,
            usize::from(cursor_y - self.scroll),
            to_u16(wrapped.cursor_column),
        )
    }

    /// The cursor's shape: a bar to insert text, as in the command line, and a block for vi's
    /// other modes.
    fn cursor_shape(&self) -> SetCursorStyle {
        match self.editor.mode() {
            _ if self.command_line.is_some() => SetCursorStyle::SteadyBar,
            Mode::Insert => SetCursorStyle::SteadyBar,
            Mode::Normal | Mode::Visual | Mode::VisualLine => SetCursorStyle::SteadyBlock,
        }
    }

    /// The label row's line: the label, and at the right edge the keys of a half-typed vi
    /// command, where vim's `showcmd` shows them, so nothing else on the row moves when they
    /// appear or go, and the question view's progress through the batch. On a screen too narrow
    /// for both, the keys follow the label after a space, and wrap onto the next row.
    fn label_line(&self, view: Option<&View>) -> String {
        let pending = self.editor.pending();
        let pending = (!pending.is_empty()).then(|| format!("{STATUS_COLOR}{pending}"));
        let right: Vec<String> = [pending, view.and_then(progress)]
            .into_iter()
            .flatten()
            .collect();
        let right = right.join(" ");
        if right.is_empty() {
            return self.label.clone();
        }
        let room = usize::from(self.layout.width)
            .saturating_sub(visible_width(&self.label) + visible_width(&right))
            .max(1);
        format!("{}{}{right}", self.label, " ".repeat(room))
    }

    /// Draws the question of `view` in `rows`: the question above the label row, and on a
    /// question whose options have previews, the preview of the option the cursor is on from the
    /// top of the screen, between an edge titled with the option and a rule above the question,
    /// as fzf's preview window with a label, so only the preview changes as the cursor moves.
    /// Below the label row go the options, on the bar, and the user's own answer right above the
    /// box. Each option and preview line takes one row, and loses its middle to `…` when it is
    /// wider than the screen, since a line that wrapped would push the rows below it out of
    /// place. The limit of noisy lines is for the scrollback, which this screen never reaches.
    fn draw_question(&self, out: &mut Vec<u8>, view: &View, rows: ViewRows) {
        let width = self.layout.width;
        put!(out, Clear(ClearType::All));
        let columns = NonZeroUsize::new(usize::from(width)).unwrap_or(NonZeroUsize::MIN);
        let preview = picker::preview(&view.choices, &self.pick);
        // An edge across the screen, with `title` in it, as fzf's preview label
        let rule = |title: &str| {
            let title = if title.is_empty() {
                String::new()
            } else {
                format!(
                    "── {}{RESET}{STATUS_COLOR} ",
                    cut(&format!("{BOLD}{title}"), columns)
                )
            };
            let fill = "─".repeat(usize::from(width).saturating_sub(visible_width(&title)));
            format!("{STATUS_COLOR}{title}{fill}")
        };
        let mut lines = Vec::new();
        if preview.is_some() {
            lines.push(rule(""));
        }
        lines.extend(view.lines.iter().cloned());
        let top = draw_lines_above(out, &lines, width, rows.label);
        if let Some(preview) = preview {
            put_line(out, 0, &rule(preview.title.as_deref().unwrap_or_default()));
            // The rows between the edge and the rule above the question, less a blank row
            // above the rule. A cut keeps the last of them for the …
            let room = usize::from(top.saturating_sub(2));
            let is_cut = preview.lines.len() > room;
            let shown = if is_cut {
                room.saturating_sub(1)
            } else {
                preview.lines.len()
            };
            let mut lines: Vec<String> = preview.lines[..shown]
                .iter()
                .map(|line| cut(line, columns))
                .collect();
            if is_cut && room > 0 {
                lines.push(format!("{STATUS_COLOR}… Ctrl+O shows it whole"));
            }
            for (row, line) in (1..).zip(lines) {
                put_line(out, row, &line);
            }
        }
        let submitted = view
            .is_submitted
            .then(|| self.pick.chosen(view.form, &self.text()));
        let (options, own) = picker::lines(
            &view.choices,
            &view.own_label,
            view.form,
            &self.pick,
            submitted.as_ref(),
        );
        // The rows the options leave empty stay on the bar, above the user's own answer
        let mut options = options.iter();
        for row in rows.options..rows.own {
            let line = options
                .next()
                .map_or_else(String::new, |line| cut(line, columns));
            put_bar_line(out, row, &line);
        }
        put_bar_line(out, rows.own, &cut(&own, columns));
    }

    /// Lays the screen out for its new size, which the frame after it draws. The main screen
    /// needs nothing else, since a frame finds the live rows wherever the resize moved them.
    fn resize(&mut self, width: u16, height: u16) {
        self.layout = Layout::new(width, height, self.input_rows);
    }

    /// The command line, in the label row's state while the user writes it, as on vim's.
    fn command_line(&self) -> Option<String> {
        (self.command_line.as_ref()).map(|line| format!("{RESET}:{}", line.text()))
    }

    /// What the label row says while a key waits for its second press, in the color of a
    /// warning.
    fn confirmation(&self) -> Option<String> {
        self.confirming
            .as_ref()
            .map(|first| format!("{ORANGE}{} again {}", keys::name(&first.key), first.loss))
    }

    /// The label row: for a message, `››` with the budgets, then the activity, as AGENTS.md's
    /// rules for the label row group them. The budgets are the context's level, marked and red
    /// on a cold cache, and the levels of the plan's usage limits. The activity is the questions
    /// that wait, in the color of a warning since only the user moves them on, the tasks that run
    /// in the background, and what Claude does, `idle` too, so the user always sees whether it is
    /// their turn. For an answer, the question; at the end, the work Claude still does.
    fn make_label(&self) -> String {
        let shared = lock(&self.shared);
        match &shared.phase {
            Phase::Writing { waiting } => {
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
                let budgets = format!(
                    "{level_color}{cold_mark}T{} {} {}",
                    show_level(level),
                    limit_indicator('S', shared.limits.session().map(limit_level)),
                    limit_indicator('W', shared.limits.week().map(limit_level)),
                );
                let activity = format!(
                    "{} {}",
                    waiting_indicator(*waiting),
                    background_indicator(shared.output.background_tasks().len()),
                );
                let indicators = format!("{}{budgets} {STATUS_COLOR}│ {activity}", marks());
                let state = (self.confirmation())
                    .or_else(|| self.command_line())
                    .or_else(|| describe_work(&shared.output))
                    .unwrap_or_else(|| "idle".to_string());
                format!("{indicators} {STATUS_COLOR}│ {state}")
            }
            Phase::Answering(view) => {
                let hint = if let Some(confirmation) = self.confirmation() {
                    confirmation
                } else if self.is_empty_warned {
                    let text = match view.form {
                        Form::One => "write your answer, or Esc goes back",
                        _ => "Space ticks one or more",
                    };
                    format!("{ORANGE}{text}")
                } else if view.is_submitted {
                    format!("{GREEN}submitted{STATUS_COLOR}, Enter edits it")
                } else {
                    picker::hint(
                        view.form,
                        &self.pick,
                        view.choices.len(),
                        view.sends_at_once,
                    )
                };
                format!(
                    "{} {BOLD}{}{RESET} {STATUS_COLOR}│ {hint}",
                    marks(),
                    view.label
                )
            }
            Phase::Ending => {
                let output = &shared.output;
                let tasks = output.background_tasks();
                let descriptions: Vec<&str> =
                    tasks.iter().map(|task| task.description.as_str()).collect();
                let background = (!descriptions.is_empty())
                    .then(|| format!("background: {}", descriptions.join(", ")));
                let parts: Vec<String> = ["ending".to_string()]
                    .into_iter()
                    .chain(describe_work(output))
                    .chain(background)
                    .collect();
                let confirmation = self
                    .confirmation()
                    .map_or_else(String::new, |confirmation| format!(" │ {confirmation}"));
                format!(
                    "{} {STATUS_COLOR}{}{confirmation}",
                    marks(),
                    parts.join(" │ ")
                )
            }
        }
    }
}

/// A key pressed once that waits for its second press in a row, with what it loses.
struct Confirming {
    key: KeyEvent,
    loss: String,
    /// The key's action, whose loss the session's state can take away meanwhile, as when Claude
    /// Code tells the cache is new. None for an answer's key, whose loss only keys change.
    action: Option<Action>,
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
    /// The layout of a screen `width` columns wide and `height` rows high, with an input box of
    /// `input_rows` rows when the screen has them.
    fn new(width: u16, height: u16, input_rows: NonZeroU16) -> Self {
        // A short screen keeps a row to type in first, then the label row, then the
        // conversation's rows, and takes the rest from the box
        let label_rows = u16::from(height >= 2);
        let input_rows = input_rows
            .get()
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

/// Where the last frame left the cursor among the live rows, which the next frame climbs from.
struct Drawn {
    /// The live rows above the cursor's.
    lines_above_cursor: u16,
    /// The screen's width then, which no live row was wider than.
    width: u16,
}

/// The rows of the line Claude still writes, the text after the last line break in `pending`,
/// as the terminal will wrap it on a screen `width` wide once it ends, so it moves to the
/// conversation as it shows. At most `room` rows show: a taller line shows its end, after a row
/// that says its start is cut, and shows whole once it ends.
fn open_line_rows(pending: &str, width: usize, room: usize) -> Vec<String> {
    let open = pending
        .rfind('\n')
        .map_or(pending, |end| &pending[end + 1..]);
    if open.is_empty() || room == 0 {
        return Vec::new();
    }
    let rows = terminal_rows(open, width);
    let rows: Vec<String> = rows
        .into_iter()
        .map(|row| format!("{row}{RESET}"))
        .collect();
    if rows.len() <= room {
        return rows;
    }
    let shown = room - 1;
    let cut = format!("{STATUS_COLOR}… its start shows once it ends{RESET}");
    [vec![cut], rows[rows.len() - shown..].to_vec()].concat()
}

/// The rows the terminal wraps `line` onto at `width` columns, each after the color codes the
/// rows before it set, so that printed on rows of their own they show as the line would. A
/// character too wide for the rest of a row starts the next row.
///
/// Assumes the terminal gives each character the width unicode-width gives it, and that the
/// line holds no tab, which moves the cursor to a tab stop. A character the terminal gives
/// another width, or a tab, ends a row elsewhere, which the terminal then wraps.
fn terminal_rows(line: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut starts = vec![0];
    let mut column = 0;
    let mut characters = line.char_indices();
    while let Some((index, character)) = characters.next() {
        if character == '\x1b' {
            // Every sequence a line holds is a CSI, which ends with a letter and takes no column
            characters.find(|(_, next)| next.is_ascii_alphabetic());
            continue;
        }
        let character_width = character.width().unwrap_or(0);
        if column + character_width > width && column > 0 {
            starts.push(index);
            column = 0;
        }
        column += character_width;
    }
    let ends = starts.iter().skip(1).copied().chain([line.len()]);
    starts
        .iter()
        .zip(ends)
        .map(|(&start, end)| format!("{}{}", colors_at(line, start), &line[start..end]))
        .collect()
}

/// `line` on the bar, whose background fills the row. A reset inside the line ends the bar,
/// which then starts again.
fn bar_row(line: &str) -> String {
    let bar = start_bar_row();
    format!(
        "{bar}{}{RESET}",
        line.replace(RESET, &format!("{RESET}{bar}"))
    )
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

pub(crate) fn write_out(out: &[u8]) -> Result<(), String> {
    let mut stdout = io::stdout();
    stdout
        .write_all(out)
        .and_then(|()| stdout.flush())
        .map_err(|error| format!("cannot write to the terminal: {error}"))
}

pub(crate) fn cannot_read_terminal(error: &io::Error) -> String {
    format!("cannot read the terminal: {error}")
}

/// The last line of the session, on the bar, with the session's id once Claude Code told it.
pub fn final_line(session_id: Option<&str>) -> String {
    let session = session_id.map_or_else(String::new, |id| format!(" │ session {id}"));
    user_line(&format!("ended{session}"))
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

/// The batch's progress in the question view: the names of its questions.
fn progress(view: &View) -> Option<String> {
    names(&view.progress, view.index, question_name)
}

/// The names of the items of one level of the question view, like the questions of a batch:
/// each item's name, which is its key, green once `done`, with the one `shown` in reverse video,
/// which ends without a reset, so the bar stays behind them. Letters keep the marks the same
/// size, and set them apart from the options' numbers. A level of one item has nothing to move
/// to, so it shows no names.
fn names(done: &[bool], shown: usize, name: fn(usize) -> char) -> Option<String> {
    if done.len() < 2 {
        return None;
    }
    let names = done.iter().enumerate().map(|(index, &is_done)| {
        let color = if is_done { GREEN } else { STATUS_COLOR };
        let (on, off) = if index == shown {
            (REVERSE, UNREVERSE)
        } else {
            ("", "")
        };
        format!("{color}{on}{}{off}", name(index))
    });
    Some(names.collect::<Vec<_>>().join(" "))
}

/// Y and how many questions wait, in the color of a warning while any waits, since only the
/// user moves them on.
fn waiting_indicator(waiting: usize) -> String {
    let color = if waiting == 0 { STATUS_COLOR } else { ORANGE };
    format!("{color}Y{}", show_count(waiting))
}

/// B and how many tasks run in the background, like a shell Claude started, in the bar's grey
/// however many run, since they need nothing from the user.
fn background_indicator(tasks: usize) -> String {
    format!("{STATUS_COLOR}B{}", show_count(tasks))
}

/// A count up to 9, and + for more, which keeps one character for the count, so the label's
/// width stays.
fn show_count(count: usize) -> String {
    if count > 9 {
        "+".to_string()
    } else {
        count.to_string()
    }
}

/// Draws `lines` so that they end right above `bottom`, after a blank row, on a screen `width`
/// wide that wraps them. Lines taller than the rows left lose their start, where `…` shows, since
/// the conversation has them whole.
fn draw_lines_above(out: &mut Vec<u8>, lines: &[String], width: u16, bottom: u16) -> u16 {
    let mut row = bottom.saturating_sub(1);
    for (index, line) in lines.iter().enumerate().rev() {
        let line_rows = rows_of(line, width);
        // A cut keeps a row for the …
        let room = if index == 0 {
            row
        } else {
            row.saturating_sub(1)
        };
        if line_rows > room {
            if row > 0 {
                put!(out, MoveTo(0, row - 1));
                out.extend(format!("{STATUS_COLOR}… Ctrl+O shows it whole{RESET}").as_bytes());
            }
            return row.saturating_sub(1);
        }
        row -= line_rows;
        put_line(out, row, line);
    }
    row
}

/// Writes `line` on `row`, from its start, and resets the colors after it, so they reach nothing
/// drawn next.
fn put_line(out: &mut Vec<u8>, row: u16, line: &str) {
    put!(out, MoveTo(0, row));
    out.extend(format!("{line}{RESET}").as_bytes());
}

/// Writes `line` on `row` on the bar, like `put_line`. A reset inside the line ends the bar, which
/// then starts again.
fn put_bar_line(out: &mut Vec<u8>, row: u16, line: &str) {
    put!(out, MoveTo(0, row));
    out.extend(bar_row(line).as_bytes());
}

/// The rows of the question view, from the bottom of the screen up: the input box, the user's own
/// answer right above it, the options above that, and the label row above them.
#[derive(Clone, Copy)]
struct ViewRows {
    label: u16,
    /// The first row of the options.
    options: u16,
    own: u16,
    box_top: u16,
}

impl ViewRows {
    /// The rows of the question view of a question of `choices` options, on a screen laid out as
    /// `layout`.
    fn new(layout: Layout, choices: usize) -> Self {
        let box_top = layout.height.saturating_sub(layout.input_rows);
        let own = box_top.saturating_sub(1);
        let options = own.saturating_sub(picker::option_rows(choices));
        ViewRows {
            label: options.saturating_sub(1),
            options,
            own,
            box_top,
        }
    }
}

/// The rows `line` takes on a screen `width` wide, which wraps it.
fn rows_of(line: &str, width: u16) -> u16 {
    let width = usize::from(width.max(1));
    u16::try_from(visible_width(line).div_ceil(width).max(1)).unwrap_or(u16::MAX)
}

/// The Esc key, which takes the box to normal mode.
fn escape() -> KeyEvent {
    KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
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
        Status::Running { tool, .. } => Some(format!("running {tool}")),
        Status::Checking { tool, .. } => Some(format!("checking {tool}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_count_keeps_one_character() {
        assert_eq!(background_indicator(0), format!("{STATUS_COLOR}B0"));
        assert_eq!(background_indicator(9), format!("{STATUS_COLOR}B9"));
        assert_eq!(background_indicator(10), format!("{STATUS_COLOR}B+"));
        assert_eq!(waiting_indicator(0), format!("{STATUS_COLOR}Y0"));
        assert_eq!(waiting_indicator(12), format!("{ORANGE}Y+"));
    }

    #[test]
    fn a_level_names_its_items_only_when_it_has_several() {
        assert_eq!(names(&[false], 0, question_name), None);
        let two = names(&[true, false], 1, question_name).expect("names");
        assert!(two.contains(&format!("{GREEN}a")));
        assert!(two.contains(&format!("{REVERSE}s")));
    }

    use crate::style::BOLD;

    #[test]
    fn a_line_that_fits_takes_one_row() {
        assert_eq!(terminal_rows("abc", 5), ["abc"]);
        assert_eq!(terminal_rows("", 5), [""]);
    }

    #[test]
    fn rows_end_where_the_terminal_wraps() {
        assert_eq!(terminal_rows("abcdefg", 3), ["abc", "def", "g"]);
        assert_eq!(terminal_rows("abcdef", 3), ["abc", "def"]);
    }

    #[test]
    fn a_wide_character_that_does_not_fit_starts_the_next_row() {
        assert_eq!(terminal_rows("ab日", 3), ["ab", "日"]);
    }

    #[test]
    fn each_row_starts_with_the_colors_set_before_it() {
        let line = format!("a{BOLD}bc{RESET}d{BOLD}efg");
        assert_eq!(
            terminal_rows(&line, 3),
            [
                format!("a{BOLD}bc{RESET}"),
                format!("d{BOLD}ef"),
                format!("{BOLD}g")
            ]
        );
    }

    #[test]
    fn the_open_line_is_the_text_after_the_last_line_break() {
        assert_eq!(
            open_line_rows("done\nabcd", 2, 9),
            [format!("ab{RESET}"), format!("cd{RESET}")]
        );
        assert_eq!(open_line_rows("done\n", 2, 9), Vec::<String>::new());
    }

    #[test]
    fn an_open_line_taller_than_the_room_shows_its_end() {
        let rows = open_line_rows("abcdef", 2, 2);
        assert_eq!(rows.len(), 2);
        assert!(rows[0].contains('…'));
        assert_eq!(rows[1], format!("ef{RESET}"));
    }
}
