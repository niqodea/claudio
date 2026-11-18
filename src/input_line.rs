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
use crate::config::Stream;
use crate::context::COLD_COLOR;
use crate::cut::{
    self, ScreenRow, Whole, colors_at, cut, cut_before_hidden, cut_path, visible_width,
};
use crate::keys::{
    self, Action, Answering, BLOCK_ROWS, Confirmed, Effect, Ending, Field, Gate, Mark, OnEnding,
    OnMessage, OnQuestion, PREVIEWED_QUESTION_ROWS, Phase, Reading, SessionKey, Tone, View,
    Writing,
};
use crate::limits::{Level, limit_level};
use crate::picker::{self, Cursor, Draft, Form, Move, Pick, question_name, request_name};
use crate::render::{Output, Status, marks, start_bar_row, user_line};
use crate::status;
use crate::style::{
    ATTENTION, DONE_OR_ADDED, FAILED_OR_REMOVED, MARK, RESET, REVERSE, STRONG, SUBDUED,
    SUBDUED_ON_BAR, UNREVERSE,
};
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
pub enum Outcome<D, E> {
    /// The key's effect on the session, which the gate let through, with the draft.
    Act(D, Confirmed<E>),
    /// The user opens the questions that wait, or goes back from them to the message, with the
    /// draft of what the user leaves.
    Questions(D),
    /// The user moves to another question, with the draft of the question left.
    Go(D, Move),
    /// Claude Code asked something, which the main thread acts on first, with the draft.
    Break(D),
}

impl<E> Outcome<(), E> {
    /// The same outcome, with `draft`.
    fn with<D>(self, draft: D) -> Outcome<D, E> {
        match self {
            Outcome::Act((), confirmed) => Outcome::Act(draft, confirmed),
            Outcome::Questions(()) => Outcome::Questions(draft),
            Outcome::Go((), to) => Outcome::Go(draft, to),
            Outcome::Break(()) => Outcome::Break(draft),
        }
    }
}

/// How a reading's keys act: on the input line, and on the session through the gate, with the
/// effects the reading decides.
trait Keys: Reading {
    /// Acts on `key`, of `action`, in `line`, with an outcome for one that ends the reading.
    fn key(
        &self,
        line: &mut InputLine,
        key: KeyEvent,
        action: Action,
    ) -> Result<Option<Outcome<(), Self::Effect>>, String> {
        line.pass_reading(self, key, action)
    }
}

impl Keys for Writing {}

impl Keys for Ending {}

impl Keys for Answering {
    /// Enter and the keys that edit and move act through the pick, and the other keys as on a
    /// message.
    fn key(
        &self,
        line: &mut InputLine,
        key: KeyEvent,
        action: Action,
    ) -> Result<Option<Outcome<(), OnQuestion>>, String> {
        let before = line.pick.clone();
        match line.press_pick(&self.view, &key, action) {
            Some(effect) => Ok(line.pass_pick(&self.view, &key, effect, before)),
            None => line.pass_reading(self, key, action),
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
    /// Whether the line Claude still writes shows among the live rows, or waits until it ends.
    stream: Stream,
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
    /// The confirmation every key passes, which holds the key pressed once that waits for its
    /// second press in a row, which the label row says.
    gate: Gate,
    layout: Layout,
    /// The first row of the text the box shows.
    scroll: u16,
    /// The conversation's text not printed yet: whole lines, which wait for the main screen and
    /// for a screen with rows to show them, then the line Claude still writes, which shows among
    /// the live rows until it ends, when the user streams by chunk.
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
        stream: Stream,
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
            stream,
            editor: Editor::new(""),
            command_line: None,
            pick: Draft::new().pick,
            is_empty_warned: false,
            gate: Gate::default(),
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

    /// Reads a message, starting from `draft`, while `waiting` questions of Claude's wait and the
    /// conversation goes on above it.
    pub fn read(
        &mut self,
        waiting: usize,
        draft: &str,
    ) -> Result<Outcome<String, OnMessage>, String> {
        self.read_text(&Writing { waiting }, draft)
    }

    /// Reads the keys of the session's end, whose box stays empty, while Claude finishes its
    /// work.
    pub fn read_ending(&mut self) -> Result<Outcome<String, OnEnding>, String> {
        self.read_text(&Ending, "")
    }

    /// Reads the box, starting from `draft`, for `reading`.
    fn read_text<R: Keys>(
        &mut self,
        reading: &R,
        draft: &str,
    ) -> Result<Outcome<String, R::Effect>, String> {
        // A command typed for another line has nothing left to act on
        self.editor.cancel();
        // The box held an answer in the question view, whose mode and cursor are the answer's,
        // and else keeps the message as the user left it
        if self.in_view || self.text() != draft {
            self.editor.replace(draft);
        }
        let outcome = self.run(reading)?;
        self.command_line = None;
        Ok(outcome.with(self.text()))
    }

    /// Reads what the user picks and writes on the question that `view` shows, starting from
    /// `draft`.
    pub fn read_question(
        &mut self,
        view: View,
        draft: Draft,
    ) -> Result<Outcome<Draft, OnQuestion>, String> {
        self.editor.cancel();
        self.editor.replace(&draft.text);
        // The box waits in normal mode while the cursor is on the list
        if draft.pick.cursor != Cursor::Writing {
            self.editor.press(&escape());
        }
        self.pick = draft.pick;
        let outcome = self.run(&Answering {
            view: Box::new(view),
        })?;
        Ok(outcome.with(Draft {
            pick: self.pick.clone(),
            text: self.text(),
        }))
    }

    /// Reads keys for `reading` until one ends the reading.
    fn run<R: Keys>(&mut self, reading: &R) -> Result<Outcome<(), R::Effect>, String> {
        let phase = reading.phase();
        let answers = matches!(phase, Phase::Answering(_));
        lock(&self.shared).phase = phase;
        // What a waiting key loses can change between readings, as with a question that arrives
        self.gate.reset();
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
                    let action = self.begin_key(&key);
                    if let Some(outcome) = reading.key(self, key, action)? {
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

    /// What `key` does, in the phase the label row shows. Any key other than an edit cancels
    /// the half-typed vi command, and the warning of an empty answer.
    fn begin_key(&mut self, key: &KeyEvent) -> Action {
        let phase = lock(&self.shared).phase.clone();
        self.is_empty_warned = false;
        let action = keys::action(&phase, key, self.input());
        if action != Action::Edit {
            self.editor.cancel();
        }
        action
    }

    /// Passes `key`, of `action`, through the gate with the effect that `reading` decides for it.
    fn pass_reading<R: Reading>(
        &mut self,
        reading: &R,
        key: KeyEvent,
        action: Action,
    ) -> Result<Option<Outcome<(), R::Effect>>, String> {
        let text = self.sent_text();
        let effect = reading.effect(action, self.stakes(&text));
        self.pass(key, action, effect)
    }

    /// Passes `key`, of `action`, through the gate with its `effect` on the session, which goes
    /// to the session once the gate lets it through. A key with none acts on the input line, and
    /// ends the wait of the key before.
    fn pass<E: Effect>(
        &mut self,
        key: KeyEvent,
        action: Action,
        effect: Option<E>,
    ) -> Result<Option<Outcome<(), E>>, String> {
        let Some(effect) = effect else {
            self.gate.reset();
            self.close_command_line(action);
            return self.act_on_line(key, action);
        };
        // The label row names what the second press loses
        let Some(confirmed) = self.gate.pass(&key, Some(action), effect) else {
            return Ok(None);
        };
        self.close_command_line(action);
        Ok(Some(Outcome::Act((), confirmed)))
    }

    /// Closes the command line for a session key, which acts on the session, and for the
    /// command it runs.
    fn close_command_line(&mut self, action: Action) {
        if matches!(action, Action::Session(_) | Action::Run) {
            self.command_line = None;
        }
    }

    /// Acts on the input line for `key`, of `action`, which has no effect on the session, with
    /// an outcome for one that ends the reading.
    fn act_on_line<E>(
        &mut self,
        key: KeyEvent,
        action: Action,
    ) -> Result<Option<Outcome<(), E>>, String> {
        let phase = lock(&self.shared).phase.clone();
        match action {
            Action::CommandLine => self.command_line = Some(Editor::new("")),
            Action::Session(SessionKey::Questions) => return Ok(Some(Outcome::Questions(()))),
            Action::Session(SessionKey::ShowFullView) => self.show_full_view()?,
            Action::Session(SessionKey::ShowStatus) => self.show_status()?,
            // The question view's text is the user's own answer, which the box holds only while
            // the cursor is on it
            Action::Session(SessionKey::Edit) => match &phase {
                Phase::Answering(Answering { view })
                    if view.is_submitted || self.pick.cursor != Cursor::Writing => {}
                _ => self.edit_in_editor()?,
            },
            Action::Edit => match &mut self.command_line {
                Some(_) if key.code == KeyCode::Esc => self.command_line = None,
                Some(line) => line.press(&key),
                None => self.editor.press(&key),
            },
            // A key with nothing to act on, like Ctrl+C with nothing to stop or Enter on an empty
            // box, does nothing
            Action::Session(
                SessionKey::Stop
                | SessionKey::End
                | SessionKey::Clear
                | SessionKey::Down
                | SessionKey::Up,
            )
            | Action::Send
            | Action::Run
            | Action::Ignore => {}
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

    /// What Enter sends or runs: the command line's text while it is open, and else the box's.
    fn sent_text(&self) -> String {
        (self.command_line.as_ref()).map_or_else(|| self.text(), Editor::text)
    }

    /// What the session holds that a press can lose, with `text` that Enter sends.
    fn stakes<'a>(&self, text: &'a str) -> keys::Stakes<'a> {
        let shared = lock(&self.shared);
        keys::Stakes {
            text,
            held: shared.context.held(Instant::now()),
            is_working: shared.output.status() != Status::Idle,
        }
    }

    /// Ends the wait of a key whose loss went away, so the label row never asks for a press
    /// that loses nothing.
    fn drop_lost_confirmation(&mut self) {
        let phase = lock(&self.shared).phase.clone();
        let text = self.sent_text();
        let stakes = self.stakes(&text);
        self.gate.drop_lost(&phase, stakes);
    }

    /// Presses `key` of `action` on the pick of the question that `view` shows, for Enter, an
    /// edit and a move among the options, and returns what else it does, or none for a key the
    /// pick does not take.
    fn press_pick(
        &mut self,
        view: &View,
        key: &KeyEvent,
        action: Action,
    ) -> Option<picker::Effect> {
        let choices = view.choices.len();
        match action {
            Action::Send | Action::Edit if view.is_submitted => Some(Pick::press_submitted(key)),
            Action::Send | Action::Edit => {
                Some((self.pick).press(key, &view.choices, view.form, self.input()))
            }
            Action::Session(SessionKey::Down) if !view.is_submitted => {
                Some(self.pick.move_down(choices, view.form))
            }
            Action::Session(SessionKey::Up) if !view.is_submitted => {
                Some(self.pick.move_up(choices))
            }
            _ => None,
        }
    }

    /// Carries out the pick's `effect` of `key`, besides changing the pick: Enter's submit passes
    /// the gate, and every other effect ends the wait of the key before.
    fn pass_pick(
        &mut self,
        view: &View,
        key: &KeyEvent,
        effect: picker::Effect,
        before: Pick,
    ) -> Option<Outcome<(), OnQuestion>> {
        let outcome = match effect {
            picker::Effect::Submit => return self.pass_submit(view, key, before),
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
            picker::Effect::Back => Some(Outcome::Questions(())),
            picker::Effect::Go(to) => Some(Outcome::Go((), to)),
        };
        self.gate.reset();
        outcome
    }

    /// Passes the submit of `key` through the gate. A submit that answers nothing, or that the
    /// gate holds back, leaves the pick as it was `before`.
    fn pass_submit(
        &mut self,
        view: &View,
        key: &KeyEvent,
        before: Pick,
    ) -> Option<Outcome<(), OnQuestion>> {
        let submit = if view.is_submitted || view.form == Form::Review {
            OnQuestion::Submit {
                risk: None,
                leaves_out_own: false,
            }
        } else {
            let text = self.text();
            let chosen = self.pick.chosen(view.form, &text);
            if chosen.is_empty() {
                self.gate.reset();
                self.pick = before;
                self.is_empty_warned = true;
                return None;
            }
            let risk = (chosen.options.iter()).find_map(|index| {
                view.choices
                    .get(*index)
                    .and_then(|choice| choice.risk.clone())
            });
            OnQuestion::Submit {
                risk,
                leaves_out_own: self.pick.leaves_out(view.form, &text),
            }
        };
        let confirmed = self.gate.pass(key, None, submit);
        if confirmed.is_none() {
            self.pick = before;
        }
        confirmed.map(|confirmed| Outcome::Act((), confirmed))
    }

    /// Puts the box in insert mode, at the end of its text, as vi's `A` on the last line.
    fn enter_box(&mut self) {
        self.editor.replace(&self.text());
    }

    /// Pastes `text` into the box, which the question view takes the cursor into first, on a
    /// question open to answers.
    fn paste(&mut self, text: &str) {
        self.gate.reset();
        // The command line holds one line
        if let Some(line) = &mut self.command_line {
            line.paste(&text.replace(|c: char| c.is_control(), " "));
            return;
        }
        let view = match &lock(&self.shared).phase {
            Phase::Answering(Answering { view }) => Some(view.clone()),
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
            Phase::Answering(Answering { view }) => format!("+{}g", view.full_view_line),
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
            Phase::Answering(Answering { view }) if self.in_view => Some(view.clone()),
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
    /// and the box. Each is a `ScreenRow`, so the live rows take as many rows of the terminal as
    /// the layout gives them, and never push the screen's top into the scrollback. Last, it climbs
    /// from the end of the last live row to the cursor's row, and moves along it to the cursor. No step names a row by its number, so the frame lands where
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
        let columns = self.columns();
        let label_row = (layout.label_rows > 0).then(|| self.label_row());
        let label_height = usize::from(label_row.is_some());
        // The command line takes the box's last rows, as many as it fills, and the message keeps
        // the rows above it
        let command = (self.command_line.as_ref()).map(|line| {
            command_rows(
                &line.text(),
                line.cursor(),
                columns,
                usize::from(layout.input_rows),
            )
        });
        let command_height = command.as_ref().map_or(0, |(rows, ..)| rows.len());
        let message_rows = layout.input_rows - u16::try_from(command_height).unwrap_or(0);
        let (mut box_rows, cursor_row_in_box, cursor_column_in_box) = if message_rows > 0 {
            self.box_rows(message_rows)
        } else {
            (Vec::new(), 0, 0)
        };
        let message_height = box_rows.len();
        let (cursor_row_in_box, cursor_column_in_box) = match command {
            Some((rows, row, column)) => {
                box_rows.extend(rows);
                (message_height + row, column)
            }
            None => (cursor_row_in_box, cursor_column_in_box),
        };
        let open_rows = if shows_conversation && self.stream == Stream::Chunk {
            let room = usize::from(layout.height).saturating_sub(label_height + box_rows.len());
            open_line_rows(&self.pending, columns, room)
        } else {
            Vec::new()
        };
        let to_u16 = |index: usize| u16::try_from(index).unwrap_or(u16::MAX);
        let cursor_line = open_rows.len() + label_height + cursor_row_in_box;
        let cursor_column = cursor_column_in_box;
        let lines: Vec<ScreenRow> = (open_rows.into_iter())
            .chain(label_row)
            .chain(box_rows)
            .collect();
        let text: Vec<&str> = lines.iter().map(ScreenRow::as_str).collect();
        out.extend(text.join("\r\n").as_bytes());
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
        let rows = ViewRows::new(layout.height, self.input_rows.get(), view.choices.len());
        self.draw_question(&mut out, view, rows);
        if layout.label_rows > 0 {
            put_bar_line(&mut out, rows.label, &self.question_label_row(view));
        }
        let (box_rows, cursor_row_in_box, cursor_column) = self.box_rows(rows.box_rows);
        for (row, line) in (rows.box_top..layout.height).zip(&box_rows) {
            put_line(&mut out, row, line);
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
    /// the text above could fill. It takes `rows` rows.
    fn box_rows(&mut self, rows: u16) -> (Vec<ScreenRow>, usize, u16) {
        let columns = self.columns();
        let wrapped = wrap::wrap(
            &self.text(),
            self.editor.cursor(),
            self.editor.selection(),
            usize::from(self.layout.width),
        );
        let to_u16 = |index: usize| u16::try_from(index).unwrap_or(u16::MAX);
        let cursor_y = to_u16(wrapped.cursor_row);
        let used_rows = to_u16(wrapped.rows.len());
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
                ScreenRow::cut(&format!("{}{text}{RESET}", start_bar_row()), columns)
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

    /// The label row: the label, and at its right end where the keys go, as `key_state` gives it,
    /// on the bar, in one row, which gives up text as `ScreenRow::spread` does: the label loses its end first, and the key state, which tells
    /// what the user is typing, goes last.
    fn label_row(&self) -> ScreenRow {
        let right = format!("{SUBDUED_ON_BAR}{}", self.key_state());
        let columns = self.columns();
        on_bar(&ScreenRow::spread(&self.label, &[right], columns), columns)
    }

    /// The question view's label row: the label, and at its right end the progress through the
    /// batch and where the keys go. Unlike the conversation's, it takes one
    /// row, with the options right under it, so it gives up text as `ScreenRow::spread` does.
    fn question_label_row(&self, view: &View) -> ScreenRow {
        let key_state = if self.pick.cursor == Cursor::Writing {
            self.key_state()
        } else {
            String::new()
        };
        let right = right_end(view, &key_state);
        ScreenRow::spread(&self.label, &right, self.columns())
    }

    /// Where the keys go, for the label row's right end: the keys of a half-typed vi command,
    /// where vim's `showcmd` shows them, then a letter for the mode, the box's vi mode or the
    /// command line, which takes the keys from the box while it is open: `N` for normal, `I` for
    /// insert, `V` for both visual modes and `C` for the command line, as vim's status lines show
    /// its command-line mode among the others, but in one letter like the label row's other
    /// indicators. The letter
    /// takes the edge, so it stays in place, and the keys grow leftward from it. It tells the mode
    /// in a character where the cursor's shape alone did not, which some terminals do not show.
    fn key_state(&self) -> String {
        let mode = match self.editor.mode() {
            _ if self.command_line.is_some() => 'C',
            Mode::Normal => 'N',
            Mode::Insert => 'I',
            Mode::Visual | Mode::VisualLine => 'V',
        };
        let pending = self.editor.pending();
        if pending.is_empty() {
            mode.to_string()
        } else {
            format!("{pending} {mode}")
        }
    }

    /// The screen's width.
    fn columns(&self) -> NonZeroUsize {
        NonZeroUsize::new(usize::from(self.layout.width)).unwrap_or(NonZeroUsize::MIN)
    }

    /// Draws the question of `view` in `rows`: the stack of the request's title and the block
    /// under it, right above the label row, and below the label row the options, on the bar,
    /// and the user's own answer right above the box. On a question whose options have previews,
    /// the block's last field is the preview of the option the cursor is on, quoted by a bar in
    /// its margin with the option's key in front of its first row, so only the preview changes as
    /// the cursor moves. Each option and preview line takes one row, and loses its end to `…`
    /// when it is wider than the screen, since a line that wrapped would push the rows below it
    /// out of place. When there are more options than rows, they scroll to keep the cursor in view.
    fn draw_question(&self, out: &mut Vec<u8>, view: &View, rows: ViewRows) {
        put!(out, Clear(ClearType::All));
        let columns = self.columns();
        let mut fields = view.fields.clone();
        if let Some(preview) = picker::preview(&view.choices, &self.pick) {
            fields.push(Field {
                mark: Mark::Quote(preview.key),
                value: preview.lines.join("\n"),
                is_path: false,
                tone: Tone::Plain,
                rows: BLOCK_ROWS - PREVIEWED_QUESTION_ROWS,
            });
        }
        let stack = stack_lines(&view.title, &fields, usize::from(rows.stack), columns);
        for (row, line) in (rows.stack_top..).zip(stack) {
            put_line(out, row, &line);
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
        // The first option shown: the first, or else as few after it as keeps the cursor in view
        let shown_rows = usize::from(rows.option_rows);
        let first = match self.pick.cursor {
            Cursor::Option(index) => (index + 1).saturating_sub(shown_rows),
            Cursor::Own | Cursor::Writing => 0,
        };
        // The rows the options leave empty stay on the bar, above the user's own answer
        let mut options = options.iter().skip(first);
        for row in rows.options..rows.own {
            let line = options
                .next()
                .map_or_else(ScreenRow::empty, |line| ScreenRow::cut(line, columns));
            put_bar_line(out, row, &line);
        }
        put_bar_line(out, rows.own, &ScreenRow::cut(&own, columns));
    }

    /// Lays the screen out for its new size, which the frame after it draws. The main screen
    /// needs nothing else, since a frame finds the live rows wherever the resize moved them.
    fn resize(&mut self, width: u16, height: u16) {
        self.layout = Layout::new(width, height, self.input_rows);
    }

    /// What the label row says while a key waits for its second press, in the color of a
    /// warning.
    fn confirmation(&self) -> Option<String> {
        self.gate.label().map(|label| format!("{ATTENTION}{label}"))
    }

    /// The label row: for a message, `››` with the budgets, then the activity, as AGENTS.md's
    /// rules for the label row group them. The budgets are the context's level, marked and red
    /// on a cold cache, and the levels of the plan's usage limits. The activity is the questions
    /// that wait, in the color of a warning since only the user moves them on, the tasks that run
    /// in the background, the subagents that run, and what Claude does, `idle` too, so the user always sees whether it is
    /// their turn. For an answer, the question; at the end, the work Claude still does.
    fn make_label(&self) -> String {
        let shared = lock(&self.shared);
        match &shared.phase {
            Phase::Writing(Writing { waiting }) => {
                let now = Instant::now();
                let level = shared.context.level();
                let is_cold = shared.context.is_cold(now);
                // On a cold cache, the space in front of T becomes *, which shows the cold cache
                // without color and keeps the label's width
                let (cold_mark, level_color) = if is_cold {
                    ("*", COLD_COLOR)
                } else {
                    (" ", SUBDUED_ON_BAR)
                };
                let budgets = format!(
                    "{level_color}{cold_mark}T{} {} {}",
                    show_level(level),
                    limit_indicator('S', shared.limits.session().map(limit_level)),
                    limit_indicator('W', shared.limits.week().map(limit_level)),
                );
                let output = &shared.output;
                // A subagent counts in A alone, as it runs in the background or not
                let commands = (output.background_tasks().iter())
                    .filter(|task| !task.is_agent())
                    .count();
                let activity = format!(
                    "{} {} {}",
                    waiting_indicator(*waiting),
                    background_indicator(commands),
                    agents_indicator(output.running_agents()),
                );
                let indicators = format!("{}{budgets} {SUBDUED_ON_BAR}│ {activity}", marks());
                let state = (self.confirmation())
                    .or_else(|| describe_work(&shared.output))
                    .unwrap_or_else(|| "idle".to_string());
                format!("{indicators} {SUBDUED_ON_BAR}│ {state}")
            }
            Phase::Answering(Answering { view }) => {
                let hint = if let Some(confirmation) = self.confirmation() {
                    confirmation
                } else if self.is_empty_warned {
                    let text = match view.form {
                        Form::One => "write your answer, or Esc goes back",
                        _ => "Space ticks one or more",
                    };
                    format!("{ATTENTION}{text}")
                } else if view.is_submitted {
                    format!("{DONE_OR_ADDED}submitted{SUBDUED_ON_BAR}, Enter edits it")
                } else {
                    picker::hint(
                        view.form,
                        &self.pick,
                        view.choices.len(),
                        view.sends_at_once,
                    )
                };
                format!(
                    "{} {STRONG}{}{RESET} {SUBDUED_ON_BAR}│ {hint}",
                    marks(),
                    header(&view.label)
                )
            }
            Phase::Ending(Ending) => {
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
                    "{} {SUBDUED_ON_BAR}{}{confirmation}",
                    marks(),
                    parts.join(" │ ")
                )
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

/// What starts the command line, a word rather than vim's lone `:`, so the row says what it holds
/// whatever the box holds, as Emacs's `M-x` does.
const COMMAND_PROMPT: &str = "command: ";

/// The command line's rows, on the bar, as vim's command line at the screen's bottom: the prompt
/// and the command `text` without its `/`, wrapped at `columns`, at most `rows` of them, those
/// around the `cursor`, with the row and the column of the cursor among them.
fn command_rows(
    text: &str,
    cursor: usize,
    columns: NonZeroUsize,
    rows: usize,
) -> (Vec<ScreenRow>, usize, u16) {
    let prompt_length = COMMAND_PROMPT.chars().count();
    let wrapped = wrap::wrap(
        &format!("{COMMAND_PROMPT}{text}"),
        prompt_length + cursor,
        None,
        columns.get(),
    );
    let rows = rows.min(wrapped.rows.len());
    let first = (wrapped.cursor_row + 1).saturating_sub(rows);
    let lines = wrapped.rows[first..first + rows]
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let text = match row.text.strip_prefix(COMMAND_PROMPT.trim_end()) {
                Some(rest) if first + index == 0 => {
                    format!("{MARK}{}{RESET}{rest}", COMMAND_PROMPT.trim_end())
                }
                _ => row.text.clone(),
            };
            ScreenRow::cut(&bar_row(&text), columns)
        })
        .collect();
    (
        lines,
        wrapped.cursor_row - first,
        u16::try_from(wrapped.cursor_column).unwrap_or(u16::MAX),
    )
}

/// The rows of the line Claude still writes, the text after the last line break in `pending`,
/// as the terminal will wrap it on a screen `width` wide once it ends, so it moves to the
/// conversation as it shows. At most `room` rows show: a taller line shows its end, after a row
/// that says its start is cut, and shows whole once it ends.
fn open_line_rows(pending: &str, columns: NonZeroUsize, room: usize) -> Vec<ScreenRow> {
    let open = pending
        .rfind('\n')
        .map_or(pending, |end| &pending[end + 1..]);
    if open.is_empty() || room == 0 {
        return Vec::new();
    }
    let rows = terminal_rows(open, columns.get());
    let (cut, shown) = if rows.len() <= room {
        (None, &rows[..])
    } else {
        let cut = format!("{SUBDUED_ON_BAR}… its start shows once it ends");
        (Some(cut), &rows[rows.len() - (room - 1)..])
    };
    (cut.iter().chain(shown))
        .map(|row| ScreenRow::cut(&format!("{row}{RESET}"), columns))
        .collect()
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

/// `row` on the bar, as `bar_row` draws it, whose codes take no column.
fn on_bar(row: &ScreenRow, columns: NonZeroUsize) -> ScreenRow {
    ScreenRow::cut(&bar_row(row.as_str()), columns)
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
    let color = level.and_then(Level::color).unwrap_or(SUBDUED_ON_BAR);
    format!("{color}{letter}{}", show_level(level.map(Level::to_number)))
}

/// The versions of the question view's label row's right end, from whole to a lone `…`: the
/// progress, as the names of the questions of the request shown, then those of the requests
/// that wait, then the place of the request shown, each only when its level has more than one
/// item, and last where the keys go, `key_state`, as `InputLine::key_state` gives it. Each
/// version after the first hides one more group from the left, whole, behind `…`, so a name
/// never shows cut, and the key state, which tells what the user is typing, goes last.
/// Ref: <https://github.com/nvim-lualine/lualine.nvim/wiki/Component-snippets>,
/// whose status line hides whole components on a narrow window
fn right_end(view: &View, key_state: &str) -> Vec<String> {
    let questions = names(&view.progress, view.index, question_name);
    let requests = request_names(view);
    let place = request_place(view);
    let key_state = (!key_state.is_empty()).then(|| key_state.to_string());
    // The questions change with each request, so they go left of the requests, where they move
    // nothing when they change
    let groups: Vec<(&str, String)> = [
        (" │ ", questions),
        (" │ ", requests),
        (" ", place),
        (" ", key_state),
    ]
    .into_iter()
    .filter_map(|(gap, group)| Some((gap, group?)))
    .collect();
    if groups.is_empty() {
        return Vec::new();
    }
    let joined = |groups: &[(&str, String)]| {
        (groups.iter().enumerate())
            .map(|(index, (gap, group))| {
                let gap = if index == 0 { "" } else { gap };
                format!("{SUBDUED_ON_BAR}{gap}{group}")
            })
            .collect::<Vec<_>>()
            .concat()
    };
    let hidden = |from: usize| match &groups[from..] {
        [] => format!("{SUBDUED_ON_BAR}…"),
        rest => format!("{SUBDUED_ON_BAR}… {}", joined(rest)),
    };
    [joined(&groups)]
        .into_iter()
        .chain((1..=groups.len()).map(hidden))
        .collect()
}

/// The names of the requests that wait, as for the questions of a batch, where those after the
/// fourth, which have no key, show as how many they are, like `+2`, in reverse video when one of
/// them shows.
fn request_names(view: &View) -> Option<String> {
    let count = view.requests.len();
    let keyed = count.min(picker::REQUEST_KEYS.len());
    let mut shown = names(&view.requests[..keyed], view.request_index, |index| {
        request_name(index).expect("a named request")
    })?;
    if count > keyed {
        let (on, off) = if view.request_index >= keyed {
            (REVERSE, UNREVERSE)
        } else {
            ("", "")
        };
        let rest = count - keyed;
        shown = format!("{shown} {SUBDUED_ON_BAR}{on}+{rest}{off}");
    }
    Some(shown)
}

/// The place of the request shown among those that wait, like `2/3`, when more than one waits,
/// its number padded to the width of the count, so the place keeps one width from one request
/// to the next.
fn request_place(view: &View) -> Option<String> {
    let count = view.requests.len();
    (count > 1).then(|| {
        format!(
            "{:>width$}/{count}",
            view.request_index + 1,
            width = count.to_string().len()
        )
    })
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
        let color = if is_done {
            DONE_OR_ADDED
        } else {
            SUBDUED_ON_BAR
        };
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
    let color = if waiting == 0 {
        SUBDUED_ON_BAR
    } else {
        ATTENTION
    };
    format!("{color}Y{}", show_count(waiting))
}

/// B and how many tasks run in the background, like a shell Claude started, in the bar's grey
/// however many run, since they need nothing from the user.
fn background_indicator(tasks: usize) -> String {
    format!("{SUBDUED_ON_BAR}B{}", show_count(tasks))
}

/// A and how many subagents run, in the foreground or in the background, in the bar's grey, as
/// for B.
fn agents_indicator(agents: usize) -> String {
    format!("{SUBDUED_ON_BAR}A{}", show_count(agents))
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

/// The columns a field's name takes at most, cut at its end when longer.
const NAME_WIDTH: usize = 10;

/// The columns of a field's name, the `:` and the space after it, so that every value shown by
/// name starts in the same column.
const NAME_COLUMNS: usize = NAME_WIDTH + ": ".len();

/// The columns of a question's header, as Claude Code limits it, which a call's tool takes too.
/// Ref: `AskUserQuestionInput` in `sdk-tools.d.ts` of the npm package
/// @anthropic-ai/claude-agent-sdk, which asks Claude for a header of at most 12 characters
const HEADER_COLUMNS: usize = 12;

/// `label`, a question's header or a call's tool, in `HEADER_COLUMNS` columns: padded when
/// shorter, and cut at its end when longer, since Claude can write a longer one, so the hint
/// after it starts in the same column on every request.
fn header(label: &str) -> String {
    let columns = NonZeroUsize::new(HEADER_COLUMNS).unwrap_or(NonZeroUsize::MIN);
    let label = cut(label, columns);
    let pad = " ".repeat(HEADER_COLUMNS.saturating_sub(visible_width(&label)));
    format!("{label}{pad}")
}

/// The columns a field's mark takes in front of its value: 2 for a sign, `NAME_COLUMNS` for a
/// name, none for no mark, and for a quote, whose lines are cut rather than wrapped, its margin.
fn mark_columns(mark: &Mark) -> usize {
    match mark {
        Mark::None => 0,
        Mark::Quote(key) => quote_margin(key.as_deref(), true).1,
        Mark::Sign { .. } => 2,
        Mark::Name(_) => NAME_COLUMNS,
    }
}

/// A title across a screen `width` wide: `title` between two dashes, then dashes to the edge,
/// as fzf labels its preview window.
fn title_line(title: &str, width: NonZeroUsize) -> String {
    let room = width.get().saturating_sub("──  ".len());
    let room = NonZeroUsize::new(room).unwrap_or(NonZeroUsize::MIN);
    let title = format!(
        "── {}{RESET}{SUBDUED} ",
        cut(&format!("{STRONG}{title}"), room)
    );
    let fill = "─".repeat(width.get().saturating_sub(visible_width(&title)));
    format!("{SUBDUED}{title}{fill}")
}

/// The rows of a field on a screen `columns` wide that its value takes, as `block_lines` shows
/// it: none for an empty value, its lines for a quote, and its rows wrapped for any other.
fn used_rows(field: &Field, columns: NonZeroUsize) -> usize {
    match &field.mark {
        _ if field.value.is_empty() => 0,
        Mark::Quote(_) => field.value.lines().count(),
        _ if field.rows == 1 => 1,
        mark => word_rows(
            field.value.trim_end_matches('\n'),
            columns.get().saturating_sub(mark_columns(mark)),
        )
        .len(),
    }
}

/// The margin of a quote's row and its width: the key of the option the quote previews in
/// front of the first row, which takes the style of the key under the cursor in the options, and
/// on every row a bar, which stays in the same column on each, as `2▎ `.
fn quote_margin(key: Option<&str>, is_first: bool) -> (String, usize) {
    let key_width = key.map_or(1, |key| key.width().max(1));
    let key = match key {
        Some(key) if is_first => format!("{STRONG}{key}{RESET}"),
        _ => " ".repeat(key_width),
    };
    (format!("{key}{SUBDUED}▎{RESET} "), key_width + 2)
}

/// The `field.rows` rows of a quote, the preview of an option: its lines, each cut to a row, with
/// a bar in their margin, as a long value does, ending in a row that counts the lines left out.
/// On one row, which has no room to count, its first line ends in `…`, as a value of one row
/// does, which says more is hidden.
fn quoted_lines(key: Option<&str>, field: &Field, columns: NonZeroUsize) -> Vec<String> {
    let content: Vec<&str> = field.value.lines().collect();
    let (_, margin_width) = quote_margin(key, true);
    let room =
        NonZeroUsize::new(columns.get().saturating_sub(margin_width)).unwrap_or(NonZeroUsize::MIN);
    let rows: Vec<String> = if field.rows == 1 && content.len() > 1 {
        vec![cut_before_hidden(content[0], room)]
    } else if content.len() > field.rows {
        let shown = field.rows - 1;
        let more = cut::more_lines(content.len() - shown, Whole::FullView);
        (content[..shown].iter())
            .map(|line| cut(line, room))
            .chain([cut(&format!("{SUBDUED}{more}"), room)])
            .collect()
    } else {
        content.iter().map(|line| cut(line, room)).collect()
    };
    let mut lines: Vec<String> = (rows.iter().enumerate())
        .map(|(index, row)| format!("{}{row}{RESET}", quote_margin(key, index == 0).0))
        .collect();
    lines.resize(field.rows, String::new());
    lines
}

/// The rows of the stack above the label row, `rows` of them: the request's title, then the
/// block of `fields` under it, whose last row swallows what does not fit, by `cut::fit`. The
/// title goes last, as the top of the stack: on one row, the row swallows it too, and says so.
fn stack_lines(
    title: &str,
    fields: &[Field],
    rows: usize,
    columns: NonZeroUsize,
) -> Vec<ScreenRow> {
    let lines = match rows {
        0 => Vec::new(),
        1 => {
            let request = cut::the_request(Whole::FullView);
            vec![format!(
                "{}{RESET}",
                cut(&format!("{SUBDUED}{request}"), columns)
            )]
        }
        _ => {
            let mut lines = vec![title_line(title, columns)];
            lines.extend(block_lines(fields, rows - 1, columns));
            lines
        }
    };
    (lines.iter())
        .map(|line| ScreenRow::cut(line, columns))
        .collect()
}

/// The `rows` rows of the block under the title, `fields` fitted in them by `cut::fit`: each
/// field's mark, then its value from the column `mark_columns` gives, so that a value starts in
/// the same column on every call of a tool, in the rows the field takes. A value of one row
/// shows its line breaks as `↵`, and when it is too wide, loses its end to `…`, or its middle if
/// it is a path. A value of more rows wraps, and when it still does not fit, its last row counts
/// the rows left out. A quote shows its lines by `quoted_lines`. Rows a value leaves empty stay
/// empty, so the block always takes `rows` rows.
fn block_lines(fields: &[Field], rows: usize, columns: NonZeroUsize) -> Vec<String> {
    let fields = cut::fit(fields, rows, |field| used_rows(field, columns));
    let mut lines = Vec::new();
    for field in &fields {
        if let Mark::Quote(key) = &field.mark {
            lines.extend(quoted_lines(key.as_deref(), field, columns));
            continue;
        }
        let tone = match field.tone {
            Tone::Plain => RESET,
            Tone::Bold => STRONG,
            Tone::Note | Tone::Comment => SUBDUED,
            Tone::Removed => FAILED_OR_REMOVED,
            Tone::Added => DONE_OR_ADDED,
        };
        // The mark of the first row, and of each row of the value on `every_row`
        let (mark, every_row) = match &field.mark {
            Mark::None => (String::new(), false),
            Mark::Sign {
                sign,
                every_row,
                tone,
            } => {
                let color = match tone {
                    Tone::Removed => FAILED_OR_REMOVED,
                    Tone::Added => DONE_OR_ADDED,
                    _ => SUBDUED,
                };
                (format!("{color}{sign}{RESET} "), *every_row)
            }
            Mark::Name(name) if name.is_empty() => (" ".repeat(NAME_COLUMNS), false),
            Mark::Quote(_) => unreachable!("a quote shows its own rows"),
            Mark::Name(name) => {
                // The name holds its color, which the cut sets again after the `…`
                let width = NonZeroUsize::new(NAME_WIDTH).unwrap_or(NonZeroUsize::MIN);
                let name = format!("{}:", cut(&format!("{SUBDUED}{name}"), width));
                let pad = " ".repeat(NAME_COLUMNS.saturating_sub(visible_width(&name)));
                (format!("{name}{RESET}{pad}"), false)
            }
        };
        let indent = visible_width(&mark);
        let value_columns =
            NonZeroUsize::new(columns.get().saturating_sub(indent)).unwrap_or(NonZeroUsize::MIN);
        let value = field.value.trim_end_matches('\n');
        // Each row, and whether it shows the value, rather than counting the rows left out
        let rows: Vec<(String, bool)> = if field.rows == 1 {
            let line = format!("{tone}{}", value.replace('\n', "↵"));
            let line = if field.is_path {
                cut_path(&line, value_columns)
            } else {
                cut(&line, value_columns)
            };
            vec![(line, true)]
        } else {
            let wrapped = word_rows(value, value_columns.get());
            if wrapped.len() <= field.rows {
                (wrapped.into_iter())
                    .map(|row| (format!("{tone}{row}"), true))
                    .collect()
            } else {
                let shown = field.rows - 1;
                let mut rows: Vec<(String, bool)> = (wrapped[..shown].iter())
                    .map(|row| (format!("{tone}{row}"), true))
                    .collect();
                let more = cut::more_lines(wrapped.len() - shown, Whole::FullView);
                rows.push((cut(&format!("{SUBDUED}{more}"), value_columns), false));
                rows
            }
        };
        for (number, row) in
            (0..field.rows).zip(rows.into_iter().map(Some).chain(std::iter::repeat(None)))
        {
            let line = match row {
                Some((row, is_value)) if number == 0 || (every_row && is_value) => {
                    format!("{mark}{row}{RESET}")
                }
                Some((row, _)) => format!("{}{row}{RESET}", " ".repeat(indent)),
                None => String::new(),
            };
            lines.push(line);
        }
    }
    lines.resize(rows, String::new());
    lines
}

/// `text` wrapped at `width` columns between words, as prose reads, and within a word only when
/// it is wider than a row, like a long path. Each line break starts a row, which keeps the line's
/// indent, and a row that a wrap starts drops the spaces it starts with.
fn word_rows(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut rows = Vec::new();
    for line in text.split('\n') {
        let line = line.replace('\t', " ");
        // Each word with the spaces before it
        let mut words: Vec<&str> = Vec::new();
        let mut start = 0;
        let mut was_space = true;
        for (index, character) in line.char_indices() {
            let is_space = character == ' ';
            if is_space && !was_space {
                words.push(&line[start..index]);
                start = index;
            }
            was_space = is_space;
        }
        words.push(&line[start..]);
        let mut row = String::new();
        let mut row_width = 0;
        for word in words {
            let mut word = word;
            if row_width > 0 && row_width + UnicodeWidthStr::width(word) > width {
                rows.push(std::mem::take(&mut row));
                row_width = 0;
                word = word.trim_start();
            }
            for character in word.chars() {
                let character_width = character.width().unwrap_or(0);
                if row_width > 0 && row_width + character_width > width {
                    rows.push(std::mem::take(&mut row));
                    row_width = 0;
                }
                row.push(character);
                row_width += character_width;
            }
        }
        rows.push(row);
    }
    rows
}

/// Writes `line` on `row`, from its start, and resets the colors after it, so they reach nothing
/// drawn next.
fn put_line(out: &mut Vec<u8>, row: u16, line: &ScreenRow) {
    put!(out, MoveTo(0, row));
    out.extend(format!("{}{RESET}", line.as_str()).as_bytes());
}

/// Writes `line` on `row` on the bar, like `put_line`. A reset inside the line ends the bar, which
/// then starts again.
fn put_bar_line(out: &mut Vec<u8>, row: u16, line: &ScreenRow) {
    put!(out, MoveTo(0, row));
    out.extend(bar_row(line.as_str()).as_bytes());
}

/// The most rows of the stack above the label row: the request's title and the block under it.
const STACK_ROWS: usize = 1 + BLOCK_ROWS;

/// The rows of the question view, from the bottom of the screen up: the input box, the user's own
/// answer right above it, the options above that, the label row above them, and the stack of the
/// title and the block above it all. The rows of each part depend on the screen's height alone,
/// so no part moves from one question to the next.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ViewRows {
    /// The first row of the stack, and how many it takes.
    stack_top: u16,
    stack: u16,
    label: u16,
    /// The first row of the options, and how many they take.
    options: u16,
    option_rows: u16,
    own: u16,
    box_top: u16,
    box_rows: u16,
}

impl ViewRows {
    /// The rows of the question view of a question of `choices` options, on a screen `height`
    /// rows high, with an input box of at most `box_rows` rows. A screen too short for all of it
    /// takes rows first from the box, down to 1, then from the stack, which `cut::fit` cuts
    /// with a sign, down to 1, then from the options, which scroll to keep the cursor in view,
    /// down to 1. The label row, the user's own answer and a row of the box stay to the last,
    /// since the user answers with them.
    /// Ref: fzf's `src/terminal.go`, where the preview gives up its rows before the list
    /// Ref: <https://www.w3.org/WAI/WCAG22/Understanding/focus-not-obscured-minimum.html>, which
    /// keeps the item under the cursor in view
    fn new(height: u16, box_rows: u16, choices: usize) -> Self {
        let mut left = height;
        let mut take = |wanted: u16| {
            let taken = wanted.min(left);
            left -= taken;
            taken
        };
        // The parts in the order they get their rows, the reverse of the one they give them up in
        let label = take(1);
        let own = take(1);
        let first_box_row = take(1);
        let first_option_row = take(1);
        let first_stack_row = take(1);
        let option_rows = first_option_row + take(picker::option_rows(choices).saturating_sub(1));
        let stack = first_stack_row + take(u16::try_from(STACK_ROWS - 1).unwrap_or(u16::MAX));
        let box_rows = first_box_row + take(box_rows.saturating_sub(1));
        let box_top = height - box_rows;
        let own_row = box_top.saturating_sub(own);
        let options = own_row.saturating_sub(option_rows);
        let label_row = options.saturating_sub(label);
        ViewRows {
            stack_top: label_row.saturating_sub(stack),
            stack,
            label: label_row,
            options,
            option_rows,
            own: own_row,
            box_top,
            box_rows,
        }
    }
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
        Status::Running { tool, call } => {
            Some(format!("running {}", output.name_call(&tool, &call)))
        }
        Status::Checking { tool, call } => {
            Some(format!("checking {}", output.name_call(&tool, &call)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_line_starts_with_its_prompt() {
        let (rows, row, column) = command_rows("model sonnet", 5, columns(40), 3);
        let rows: Vec<&str> = rows.iter().map(ScreenRow::as_str).collect();
        assert_eq!(rows.len(), 1);
        assert!(
            rows[0].contains(&format!("{MARK}command:{RESET}")),
            "{:?}",
            rows[0]
        );
        assert!(
            rows[0].ends_with(&format!(" model sonnet{RESET}")),
            "{:?}",
            rows[0]
        );
        assert_eq!((row, column), (0, 14));
    }

    #[test]
    fn a_long_command_takes_more_rows_and_keeps_its_cursor() {
        // "command: " and 20 characters wrap at 10 columns onto 3 rows
        let text = "abcdefghijklmnopqrst";
        let (rows, row, column) = command_rows(text, 20, columns(10), 5);
        assert_eq!(rows.len(), 3);
        assert_eq!((row, column), (2, 9));
        let (rows, row, _) = command_rows(text, 20, columns(10), 2);
        assert_eq!(rows.len(), 2);
        assert_eq!(row, 1);
        assert!(!rows[0].as_str().contains("command"));
        let (rows, row, column) = command_rows(text, 0, columns(10), 2);
        assert!(rows[0].as_str().contains("command"));
        assert_eq!((row, column), (0, 9));
    }

    #[test]
    fn a_count_keeps_one_character() {
        assert_eq!(background_indicator(0), format!("{SUBDUED_ON_BAR}B0"));
        assert_eq!(background_indicator(9), format!("{SUBDUED_ON_BAR}B9"));
        assert_eq!(background_indicator(10), format!("{SUBDUED_ON_BAR}B+"));
        assert_eq!(waiting_indicator(0), format!("{SUBDUED_ON_BAR}Y0"));
        assert_eq!(waiting_indicator(12), format!("{ATTENTION}Y+"));
    }

    #[test]
    fn a_level_names_its_items_only_when_it_has_several() {
        assert_eq!(names(&[false], 0, question_name), None);
        let two = names(&[true, false], 1, question_name).expect("names");
        assert!(two.contains(&format!("{DONE_OR_ADDED}a")));
        assert!(two.contains(&format!("{REVERSE}s")));
    }

    #[test]
    fn requests_after_the_fourth_show_as_their_count_beside_the_questions() {
        let view = |requests: usize, request_index: usize, questions: usize| View {
            label: String::new(),
            title: String::new(),
            refusal: String::new(),
            request_index,
            requests: vec![false; requests],
            fields: vec![],
            choices: vec![],
            own_label: String::new(),
            form: Form::One,
            full_view_line: 1,
            index: 0,
            progress: vec![false; questions],
            is_submitted: false,
            sends_at_once: false,
        };
        assert_eq!(right_end(&view(1, 0, 1), ""), Vec::<String>::new());
        let plain = |text: String| {
            let mut plain = String::new();
            let mut characters = text.chars();
            while let Some(character) = characters.next() {
                if character == '\x1b' {
                    characters.by_ref().find(char::is_ascii_alphabetic);
                } else {
                    plain.push(character);
                }
            }
            plain
        };
        let six = &right_end(&view(6, 5, 2), "")[0];
        assert_eq!(plain(six.clone()), "a s │ q w e r +2 6/6");
        assert!(six.contains(&format!("{REVERSE}+2")));
        assert_eq!(plain(right_end(&view(2, 0, 1), "")[0].clone()), "q w 1/2");
        // The place keeps the width of the count, so the names keep their columns
        assert_eq!(
            plain(right_end(&view(10, 2, 1), "")[0].clone()),
            "q w e r +6  3/10"
        );
        // Each shorter version hides one more group from the left, whole
        let versions: Vec<String> = right_end(&view(6, 5, 2), "")
            .into_iter()
            .map(plain)
            .collect();
        assert_eq!(
            versions,
            ["a s │ q w e r +2 6/6", "… q w e r +2 6/6", "… 6/6", "…"]
        );
        let questions: Vec<String> = right_end(&view(1, 0, 2), "")
            .into_iter()
            .map(plain)
            .collect();
        assert_eq!(questions, ["a s", "…"]);
        // Where the keys go, a half-typed command and the mode, takes the right end, and goes last
        let typing: Vec<String> = right_end(&view(2, 0, 1), "d2 N")
            .into_iter()
            .map(plain)
            .collect();
        assert_eq!(typing, ["q w 1/2 d2 N", "… 1/2 d2 N", "… d2 N", "…"]);
        let alone: Vec<String> = right_end(&view(1, 0, 1), "I")
            .into_iter()
            .map(plain)
            .collect();
        assert_eq!(alone, ["I", "…"]);
    }

    use crate::style::STRONG;

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
        let line = format!("a{STRONG}bc{RESET}d{STRONG}efg");
        assert_eq!(
            terminal_rows(&line, 3),
            [
                format!("a{STRONG}bc{RESET}"),
                format!("d{STRONG}ef"),
                format!("{STRONG}g")
            ]
        );
    }

    #[test]
    fn the_open_line_is_the_text_after_the_last_line_break() {
        assert_eq!(
            open_rows("done\nabcd", 2, 9),
            [format!("ab{RESET}"), format!("cd{RESET}")]
        );
        assert_eq!(open_rows("done\n", 2, 9), Vec::<String>::new());
    }

    #[test]
    fn an_open_line_taller_than_the_room_shows_its_end() {
        let rows = open_rows("abcdef", 2, 2);
        assert_eq!(rows.len(), 2);
        assert!(rows[0].contains('…'));
        assert_eq!(rows[1], format!("ef{RESET}"));
    }

    #[test]
    fn the_row_for_a_cut_start_fits_the_screen() {
        // The row that stands for the start is wider than 2 columns, and is cut to them
        for row in open_line_rows("abcdef", columns(2), 2) {
            assert!(visible_width(row.as_str()) <= 2, "{:?}", row.as_str());
        }
    }

    /// The text of the rows of `fields`' block, without colors.
    fn block_text(fields: &[Field], columns: usize) -> Vec<String> {
        let columns = NonZeroUsize::new(columns).expect("some columns");
        (block_lines(fields, BLOCK_ROWS, columns).iter())
            .map(|line| plain(line))
            .collect()
    }

    fn columns(columns: usize) -> NonZeroUsize {
        NonZeroUsize::new(columns).expect("some columns")
    }

    /// The text of the open line's rows on a screen `width` wide, with `room` rows.
    fn open_rows(pending: &str, width: usize, room: usize) -> Vec<String> {
        (open_line_rows(pending, columns(width), room).iter())
            .map(|row| row.as_str().to_string())
            .collect()
    }

    /// A row's text without its colors and the spaces at its end.
    fn plain(line: &str) -> String {
        // Every sequence a row holds is a CSI, which ends with a letter
        let mut text = String::new();
        let mut characters = line.chars();
        while let Some(character) = characters.next() {
            if character == '\x1b' {
                characters.find(char::is_ascii_alphabetic);
            } else {
                text.push(character);
            }
        }
        text.trim_end().to_string()
    }

    #[test]
    fn a_value_too_wide_for_its_row_keeps_its_start_and_a_path_its_end() {
        let field = |value: &str, is_path| Field {
            mark: Mark::None,
            value: value.to_string(),
            is_path,
            tone: Tone::Plain,
            rows: 1,
        };
        let fields = [
            field("cargo test --release --all-targets", false),
            field("/home/user/src/greet.js", true),
        ];
        let columns = NonZeroUsize::new(20).expect("20 is not zero");
        let lines = block_lines(&fields, 2, columns);
        assert!(lines[0].contains("cargo test") && !lines[0].contains("targets"));
        assert!(lines[1].contains("/home") && lines[1].contains("greet.js"));
    }

    #[test]
    fn the_block_starts_values_in_the_column_of_their_mark_and_counts_the_rows_left_out() {
        let field = |mark: Mark, value: &str, rows| Field {
            mark,
            value: value.to_string(),
            is_path: false,
            tone: Tone::Plain,
            rows,
        };
        let sign = |sign, every_row| Mark::Sign {
            sign,
            every_row,
            tone: Tone::Comment,
        };
        let name = |name: &str| Mark::Name(name.to_string());
        let fields = [
            field(sign('$', false), "1\n2\n3\n4\n5", 3),
            field(sign('#', false), "a\nb", 1),
            field(name("timeout"), "600", 1),
            field(sign('?', false), "why", 1),
        ];
        assert_eq!(
            block_text(&fields, 60),
            [
                "$ 1",
                "  2",
                "  … 3 more lines · Ctrl+O shows it whole",
                "# a↵b",
                "timeout:    600",
                "? why",
                "",
                "",
            ]
        );
        // A diff's sign marks each row of the value, and a short value leaves its rows empty,
        // so each field keeps its place
        let fields = [
            field(sign('@', false), "a.rs", 1),
            field(sign('-', true), "x\ny", 2),
            field(sign('+', true), "z", 2),
            field(sign('?', false), "why", 1),
        ];
        assert_eq!(
            block_text(&fields, 40),
            ["@ a.rs", "- x", "- y", "+ z", "", "? why", "", ""]
        );
        // A name too long for its column loses its end, and the empty field that fills the
        // rows the fields leave keeps the reason on the last row
        let fields = [
            field(name("a_very_long_name"), "1", 1),
            field(Mark::None, "", 6),
            field(sign('?', false), "why", 1),
        ];
        assert_eq!(
            block_text(&fields, 40),
            ["a_very_lo…: 1", "", "", "", "", "", "", "? why"]
        );
        let columns = NonZeroUsize::new(40).expect("columns");
        let line = &block_lines(&fields, BLOCK_ROWS, columns)[0];
        assert!(
            line.contains(&format!("{SUBDUED}a_very_lo{RESET}{SUBDUED}…{RESET}:")),
            "{line:?}"
        );
    }

    #[test]
    fn a_short_screen_takes_rows_from_the_box_then_the_stack_then_the_options() {
        // The rows of the stack, the options and the box, on a screen of each height
        let parts = |height| {
            let rows = ViewRows::new(height, 3, 2);
            (rows.stack, rows.option_rows, rows.box_rows)
        };
        assert_eq!(parts(30), (9, 4, 3));
        assert_eq!(parts(18), (9, 4, 3));
        assert_eq!(parts(17), (9, 4, 2));
        assert_eq!(parts(16), (9, 4, 1));
        assert_eq!(parts(15), (8, 4, 1));
        assert_eq!(parts(8), (1, 4, 1));
        assert_eq!(parts(7), (1, 3, 1));
        assert_eq!(parts(5), (1, 1, 1));
        assert_eq!(parts(4), (0, 1, 1));
        // The parts fill the screen from the bottom, and the stack ends right above the label
        let rows = ViewRows::new(16, 3, 2);
        assert_eq!(rows.box_top, 15);
        assert_eq!(rows.own, 14);
        assert_eq!((rows.options, rows.label), (10, 9));
        assert_eq!(rows.stack_top, 0);
    }

    #[test]
    fn the_title_goes_last_and_a_stack_of_one_row_says_it_swallowed_the_request() {
        let field = |mark: Mark, value: &str| Field {
            mark,
            value: value.to_string(),
            is_path: false,
            tone: Tone::Plain,
            rows: 1,
        };
        let fields = [
            field(Mark::None, "Which style?"),
            field(Mark::Quote(Some("1".to_string())), "a\nb"),
        ];
        let columns = NonZeroUsize::new(40).expect("40 is not zero");
        let text = |rows| {
            (stack_lines("Questions", &fields, rows, columns).iter())
                .map(|line| plain(line.as_str()))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            text(3),
            [
                "── Questions ───────────────────────────",
                "Which style?",
                "1▎ a…"
            ]
        );
        assert_eq!(
            text(2),
            [
                "── Questions ───────────────────────────",
                "… Ctrl+O shows the request"
            ]
        );
        assert_eq!(text(1), ["… Ctrl+O shows the request"]);
    }

    #[test]
    fn a_value_wraps_between_words_and_within_a_word_only_wider_than_a_row() {
        assert_eq!(word_rows("one two three", 8), ["one two", "three"]);
        assert_eq!(
            word_rows("  indented\nabcdefghij", 4),
            ["  in", "dent", "ed", "abcd", "efgh", "ij"]
        );
    }
}
