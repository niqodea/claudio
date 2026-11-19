//! The session in its two phases: the conversation, which sends messages and answers to Claude
//! Code, then the ending, while Claude finishes its work.

use std::io::{self, Write};
use std::process::{Child, ChildStdin};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex};

use uuid::Uuid;

use crate::input_line::{InputLine, Outcome};
use crate::keys::{OnEnding, OnMessage, OnQuestion, TurnStop};
use crate::picker::Draft;
use crate::protocol::{Answer, Ask, AskKind, Command};
use crate::questions::{Questions, Sent};
use crate::render::{Interruption, Status};
use crate::{Shared, lock};

/// What Claude Code asks of the main thread.
pub enum Input {
    Ask(Box<Ask>),
    Unsupported {
        request_id: String,
        subtype: String,
    },
    /// The clear that waited for the tasks in the background goes, since none is left.
    HeldClear,
    /// Claude Code ended.
    Closed,
    /// Claude Code works in a way claudio was not made for, as this error says, which ends the
    /// session.
    Incompatible(String),
}

/// The session while the user talks with Claude, through Claude Code's input.
pub struct Conversation<'a> {
    session: Session,
    to_claude: ChildStdin,
    claude: &'a Child,
    request_count: u64,
}

impl<'a> Conversation<'a> {
    pub fn new(
        input: InputLine,
        shared: Arc<Mutex<Shared>>,
        inputs: Receiver<Input>,
        break_signal: Arc<AtomicBool>,
        to_claude: ChildStdin,
        claude: &'a Child,
    ) -> Self {
        Conversation {
            session: Session {
                input,
                shared,
                inputs,
                break_signal,
            },
            to_claude,
            claude,
            request_count: 0,
        }
    }

    /// Runs the conversation until Claude Code ends, or until the user has nothing more to say,
    /// which the ending follows.
    pub fn run(mut self) -> Result<Option<Ending<'a>>, String> {
        self.send(&Command::Initialize)?;
        self.send(&Command::GetSettings)?;
        self.send(&Command::GetContext)?;
        // Claude Code reads the plan's usage from the claude.ai usage endpoint, apart from the API
        // that Claude answers through. The endpoint can fail while the API works, so the
        // conversation starts before the answer arrives
        // Ref: <https://github.com/jarrodwatts/claude-hud/issues/173>, on the usage endpoint
        // answering a status line with HTTP 429
        self.send(&Command::GetUsage)?;
        let mut draft = String::new();
        // What Claude asks waits for the user to open it with Ctrl+Y, so no key typed before a
        // question showed can answer it
        let mut questions = Questions::default();
        // Whether the question view shows
        let mut is_open = false;
        loop {
            match self.session.next_input() {
                Some(Input::Ask(ask)) => {
                    let mut guard = lock(&self.session.shared);
                    let shared = &mut *guard;
                    let agent = shared.work.owner_of(&ask.tool_use_id);
                    let full_view_lines = shared.output.show_ask(&shared.work, &ask);
                    drop(guard);
                    questions.add(*ask, agent, full_view_lines);
                    continue;
                }
                Some(Input::Unsupported {
                    request_id,
                    subtype,
                }) => {
                    self.refuse(&request_id, &subtype)?;
                    continue;
                }
                Some(Input::HeldClear) => {
                    self.send(&Command::Run { command: "clear" })?;
                    continue;
                }
                Some(Input::Closed) => {
                    self.session.close_line()?;
                    return Ok(None);
                }
                Some(Input::Incompatible(error)) => return Err(error),
                None => {}
            }
            if is_open {
                let view = questions.view();
                let outcome = self.session.input.read_question(view, questions.draft())?;
                match self.act_on_question(outcome, &mut questions)? {
                    Some(still_open) => is_open = still_open,
                    None => return self.end_input(&mut questions),
                }
            } else {
                let outcome = self.session.input.read(questions.count(), &draft)?;
                match self.act_on_message(outcome, &mut questions)? {
                    Some(next) => (draft, is_open) = next,
                    None => return self.end_input(&mut questions),
                }
            }
        }
    }

    /// Acts on how the user left the message's line, and returns the message's draft and
    /// whether the question view opened, or none once the user ends the session's input. An
    /// effect goes as the key decided it, with no new look at the session's state.
    fn act_on_message(
        &mut self,
        outcome: Outcome<String, OnMessage>,
        questions: &mut Questions,
    ) -> Result<Option<(String, bool)>, String> {
        let (text, confirmed) = match outcome {
            Outcome::Act(text, confirmed) => (text, confirmed),
            Outcome::Questions(text) => return Ok(Some((text, questions.open()))),
            // Claude Code asked something, which the next round of the loop shows
            Outcome::Break(text) | Outcome::Go(text, _) => return Ok(Some((text, false))),
        };
        match confirmed.effect() {
            OnMessage::Send(sending) => {
                self.refuse_questions(questions)?;
                self.send_message(&sending.text)?;
                return Ok(Some((String::new(), false)));
            }
            // Claude Code runs a command once the turn that runs ends, and cancels the questions
            // that wait for it, as for a message
            OnMessage::Run(sending) => {
                let command = sending.text.trim();
                self.refuse_questions(questions)?;
                lock(&self.session.shared).output.show_command(command);
                self.send(&Command::Run { command })?;
            }
            OnMessage::Stop(stop) => self.stop_turn(*stop, questions)?,
            OnMessage::Kill { stop, tasks } => {
                if let Some(stop) = stop {
                    self.stop_turn(*stop, questions)?;
                }
                for task_id in tasks {
                    let request_id = self.next_request_id();
                    self.send(&Command::StopTask {
                        request_id,
                        task_id,
                    })?;
                }
            }
            // The conversation marks the clear once Claude Code tells it
            OnMessage::Clear(stop) => {
                if let Some(stop) = stop {
                    self.stop_turn(*stop, questions)?;
                }
                self.clear()?;
            }
            OnMessage::End => return Ok(None),
        }
        Ok(Some((text, false)))
    }

    /// Clears the context now, or once no task runs in the background, as `Shared::hold_clear`
    /// decides.
    fn clear(&mut self) -> Result<(), String> {
        if lock(&self.session.shared).hold_clear() {
            self.send(&Command::Run { command: "clear" })?;
        }
        Ok(())
    }

    /// Stops Claude's turn as `stop` says: the turn that waits on questions ends as they are
    /// refused, and another one with an interrupt.
    fn stop_turn(&mut self, stop: TurnStop, questions: &mut Questions) -> Result<(), String> {
        match stop {
            TurnStop::Refuse(_) => self.refuse_questions(questions),
            TurnStop::Interrupt => self.interrupt(Interruption::Stop),
        }
    }

    /// Refuses every question that waits. Claude Code cancels them for a message or a command
    /// the user sends, and claudio keeps a cancelled question, so it refuses them first.
    fn refuse_questions(&mut self, questions: &mut Questions) -> Result<(), String> {
        for ask in questions.remove_all() {
            self.reply(&ask, Answer::Deny)?;
        }
        Ok(())
    }

    /// Acts on how the user left the question shown in the question view, and returns whether
    /// the question view still shows, false to go back to the message, or none once the user ends
    /// the session's input.
    fn act_on_question(
        &mut self,
        outcome: Outcome<Draft, OnQuestion>,
        questions: &mut Questions,
    ) -> Result<Option<bool>, String> {
        let (draft, confirmed) = match outcome {
            Outcome::Act(draft, confirmed) => (draft, confirmed),
            Outcome::Questions(draft) => {
                questions.keep_draft(draft);
                return Ok(Some(false));
            }
            Outcome::Go(draft, to) => {
                questions.keep_draft(draft);
                questions.go(to);
                return Ok(Some(true));
            }
            // Claude Code asked something more, which the next round of the loop adds
            Outcome::Break(draft) => {
                questions.keep_draft(draft);
                return Ok(Some(true));
            }
        };
        Ok(Some(match confirmed.effect() {
            OnQuestion::Submit { .. } => match questions.send(draft) {
                Sent::Show => true,
                Sent::Answered {
                    answers,
                    ask,
                    answer,
                } => {
                    let mut shared = lock(&self.session.shared);
                    for (label, answer) in &answers {
                        shared.output.show_answer(label, answer);
                    }
                    drop(shared);
                    self.reply(&ask, answer)?;
                    questions.open()
                }
            },
            // Claude Code takes or refuses a call's questions together
            OnQuestion::Refuse { .. } => {
                let ask = questions.remove_shown();
                self.reply(&ask, Answer::Deny)?;
                questions.open()
            }
            OnQuestion::End => return Ok(None),
        }))
    }

    /// Refuses what Claude asks, and closes Claude Code's input, as the end of input does a
    /// program's in the terminal, since the user has nothing more to say.
    fn end_input(mut self, questions: &mut Questions) -> Result<Option<Ending<'a>>, String> {
        self.refuse_questions(questions)?;
        let Conversation {
            session,
            to_claude,
            claude,
            request_count: _,
        } = self;
        drop(to_claude);
        Ok(Some(Ending { session, claude }))
    }

    /// Tells Claude Code the answer to what it asked. A plan or a permission adds the decision
    /// to the conversation, which a call's questions do with their answers.
    fn reply(&mut self, ask: &Ask, answer: Answer) -> Result<(), String> {
        let mut guard = lock(&self.session.shared);
        let shared = &mut *guard;
        let (output, work) = (&mut shared.output, &shared.work);
        let is_decision = !matches!(ask.kind, AskKind::Questions(_));
        // The result of a refused tool repeats the refusal
        match &answer {
            Answer::Deny => {
                output.hide_result(&ask.tool_use_id);
                output.note_interrupt(Interruption::Stop);
                if is_decision {
                    output.show_decision(work, ask, "refused", None);
                }
            }
            Answer::Explain { reason } => {
                output.hide_result(&ask.tool_use_id);
                output.show_decision(work, ask, "refused", Some(reason));
            }
            Answer::Allow { .. } if is_decision => {
                output.show_decision(work, ask, "allowed", None);
            }
            Answer::Allow { .. } => {}
        }
        drop(guard);
        self.send(&Command::Answer(ask, answer))
    }

    /// Shows the message and sends it. While Claude works, every task Claude waits on moves to
    /// the background, where it goes on, and Claude Code then makes way for the message, as for
    /// one sent with its own send-now key. Claude Code alone moves only some kinds of task, and
    /// stops the turn with the others, subagents among them. Send now cancels a question that
    /// waits, and claudio keeps a cancelled question, so callers refuse every waiting question
    /// first.
    fn send_message(&mut self, text: &str) -> Result<(), String> {
        let is_working = lock(&self.session.shared).output.status() != Status::Idle;
        lock(&self.session.shared).output.show_message(text);
        let id = Uuid::new_v4().to_string();
        self.send(&Command::Message { id: &id, text })?;
        if is_working {
            let calls = lock(&self.session.shared).work.foreground_calls();
            for call in &calls {
                let request_id = self.next_request_id();
                self.send(&Command::Background {
                    request_id,
                    tool_use_id: call,
                })?;
            }
            lock(&self.session.shared)
                .output
                .note_interrupt(Interruption::Message);
            let request_id = self.next_request_id();
            self.send(&Command::SendNow {
                request_id,
                message_id: &id,
            })?;
        }
        Ok(())
    }

    /// Stops Claude's turn. An interrupt cancels a question that waits, and claudio keeps a
    /// cancelled question, so callers refuse every waiting question first.
    fn interrupt(&mut self, interruption: Interruption) -> Result<(), String> {
        lock(&self.session.shared)
            .output
            .note_interrupt(interruption);
        let request_id = self.next_request_id();
        self.send(&Command::Interrupt { request_id })
    }

    /// An id for a request to Claude Code, which its reply carries back.
    fn next_request_id(&mut self) -> String {
        self.request_count += 1;
        format!("request-{}", self.request_count)
    }

    fn refuse(&mut self, request_id: &str, subtype: &str) -> Result<(), String> {
        lock(&self.session.shared).output.show_error(&format!(
            "refused Claude Code's {subtype} request, which does not work here"
        ));
        self.send(&Command::Refuse {
            request_id,
            subtype,
        })
    }

    /// Sends the command, which Claude Code that already ended never reads. Its reader then
    /// says it closed, and the session's end says how it exited.
    fn send(&mut self, command: &Command) -> Result<(), String> {
        match self
            .to_claude
            .write_all(command.to_line().as_bytes())
            .and_then(|()| self.to_claude.flush())
        {
            Err(error) if error.kind() != io::ErrorKind::BrokenPipe => {
                Err(format!("cannot write to claude: {error}"))
            }
            _ => Ok(()),
        }
    }
}

/// The session once the user has nothing more to say: Claude Code's input closed, and Claude
/// finishes its work.
pub struct Ending<'a> {
    session: Session,
    /// Claude Code's process, which Ctrl+C interrupts.
    claude: &'a Child,
}

impl Ending<'_> {
    /// Shows Claude finish its work, until Claude Code exits once its turn and its background
    /// tasks end. Ctrl+C stops them. Claude Code fails what it asks once its input closed, so
    /// nothing it asks waits for an answer.
    ///
    /// Undocumented: [`crate::contract::a_permission_after_the_input_closes_fails`]
    pub fn run(mut self) -> Result<(), String> {
        loop {
            match self.session.next_input() {
                Some(Input::Closed) => return self.session.close_line(),
                // The ending clears nothing, so a clear that waited has no session left to clear
                Some(
                    Input::Ask(_)
                    | Input::Unsupported { .. }
                    | Input::Incompatible(_)
                    | Input::HeldClear,
                )
                | None => {}
            }
            match self.session.input.read_ending()? {
                Outcome::Act(_, confirmed) => match confirmed.effect() {
                    OnEnding::StopClaudeCode => self.interrupt()?,
                },
                // Claude Code fails what it asks once its input closed, and the ending takes no
                // other key
                Outcome::Break(_) | Outcome::Questions(_) | Outcome::Go(..) => {}
            }
        }
    }

    /// Stops Claude's turn and its background tasks with SIGINT, as Ctrl+C does a program's
    /// work in the terminal, after which Claude Code exits, its input closed.
    ///
    /// Undocumented: [`crate::contract::sigint_stops_claude_code_after_its_input_closes`]
    fn interrupt(&self) -> Result<(), String> {
        lock(&self.session.shared)
            .output
            .note_interrupt(Interruption::Stop);
        // SAFETY: kill only sends a signal. The id still names Claude Code's process: the system
        // keeps a child's id until its parent waits for it, and waiting needs exclusive access
        // to the child, which this borrow of it rules out
        if unsafe { libc::kill(self.claude.id().cast_signed(), libc::SIGINT) } == -1 {
            return Err(format!(
                "cannot stop claude: {}",
                io::Error::last_os_error()
            ));
        }
        Ok(())
    }
}

/// The terminal's side of the session: the input line, and what it shows of Claude's.
struct Session {
    input: InputLine,
    shared: Arc<Mutex<Shared>>,
    inputs: Receiver<Input>,
    break_signal: Arc<AtomicBool>,
}

impl Session {
    /// What Claude Code asks of the main thread, if anything waits.
    fn next_input(&self) -> Option<Input> {
        // A break signal raised before the line opens has nothing left to break
        self.break_signal.store(false, Ordering::Relaxed);
        match self.inputs.try_recv() {
            Ok(input) => Some(input),
            Err(TryRecvError::Disconnected) => Some(Input::Closed),
            Err(TryRecvError::Empty) => None,
        }
    }

    /// Closes the input line once Claude Code ended, so the end of the output takes its place.
    fn close_line(&mut self) -> Result<(), String> {
        self.input.close()
    }
}
