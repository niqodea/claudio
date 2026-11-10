//! The session in its two phases: the conversation, which sends messages and answers to Claude
//! Code, then the ending, while Claude finishes its work.

use std::io::{self, Write};
use std::process::{Child, ChildStdin};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use uuid::Uuid;

use crate::input_line::{InputLine, Outcome};
use crate::keys::Phase;
use crate::protocol::{Answer, Ask, AskKind, Command};
use crate::questions::{Answered, Questions};
use crate::render::{Interruption, Status};
use crate::{Shared, lock};

/// What Claude Code asks of the main thread.
pub enum Input {
    Ask(Ask),
    Unsupported {
        request_id: String,
        subtype: String,
    },
    /// Claude Code ended.
    Closed,
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
        // What Claude asks waits for the user to open it with Ctrl-Y, so no key typed before a
        // question showed can answer it
        let mut questions = Questions::default();
        // The question the question view shows, while it shows
        let mut shown: Option<usize> = None;
        loop {
            match self.session.next_input() {
                Some(Input::Ask(ask)) if matches!(ask.kind, AskKind::Permission) => {
                    lock(&self.session.shared).output.show_error(&format!(
                        "Refused {}: only auto mode is supported",
                        ask.tool_name
                    ));
                    self.reply(&ask, Answer::Deny)?;
                    continue;
                }
                Some(Input::Ask(ask)) => {
                    let full_view_lines = lock(&self.session.shared).output.show_ask(&ask);
                    questions.add(ask, full_view_lines);
                    continue;
                }
                Some(Input::Unsupported {
                    request_id,
                    subtype,
                }) => {
                    self.refuse(&request_id, &subtype)?;
                    continue;
                }
                Some(Input::Closed) => {
                    self.session.close_line()?;
                    return Ok(None);
                }
                None => {}
            }
            let outcome = match shown {
                None => self.session.input.read(
                    Phase::Writing {
                        waiting: questions.count(),
                        refusal_confirmed: questions.is_refusal_confirmed(),
                    },
                    &draft,
                )?,
                Some(index) => {
                    let question_draft = questions.draft(index).to_string();
                    self.session
                        .input
                        .read(Phase::Answering(questions.view(index)), &question_draft)?
                }
            };
            match (shown, outcome) {
                (_, Outcome::End) => return self.end_input(&mut questions),
                (None, outcome) => {
                    (draft, shown) = self.act_on_message(outcome, &mut questions)?;
                }
                (Some(index), outcome) => {
                    shown = self.act_on_question(outcome, index, &mut questions)?;
                }
            }
        }
    }

    /// Acts on how the user left the message's line, and returns the message's draft and the
    /// question the question view shows next, if it opened.
    fn act_on_message(
        &mut self,
        outcome: Outcome,
        questions: &mut Questions,
    ) -> Result<(String, Option<usize>), String> {
        Ok(match outcome {
            // A message while questions wait, or on a cold cache, stays on the input line the
            // first time, and the label says what the next Enter does
            Outcome::Send(text) => {
                if text.trim().is_empty() {
                    (String::new(), None)
                } else if questions.holds_message()
                    || lock(&self.session.shared)
                        .context
                        .holds_message(Instant::now())
                {
                    (text, None)
                } else {
                    for ask in questions.remove_all() {
                        self.reply(&ask, Answer::Deny)?;
                    }
                    self.send_message(&text)?;
                    (String::new(), None)
                }
            }
            Outcome::Questions(text) => (text, Some(0)),
            // Claude's turn waits on its questions, so stopping it refuses them
            Outcome::Stop(text) => {
                if !questions.is_empty() {
                    for ask in questions.remove_all() {
                        self.reply(&ask, Answer::Deny)?;
                    }
                } else if lock(&self.session.shared).output.status() != Status::Idle {
                    self.interrupt(Interruption::Stop)?;
                }
                (text, None)
            }
            // Claude Code asked something, which the next round of the loop shows
            Outcome::Break(text) | Outcome::Next(text) | Outcome::Previous(text) => (text, None),
            Outcome::End => unreachable!("the session ends before"),
        })
    }

    /// Acts on how the user left the question at `index` in the question view, and returns the
    /// question it shows next, none to go back to the message.
    fn act_on_question(
        &mut self,
        outcome: Outcome,
        index: usize,
        questions: &mut Questions,
    ) -> Result<Option<usize>, String> {
        let count = questions.count();
        Ok(match outcome {
            Outcome::Send(text) => match questions.answer(index, &text) {
                None => {
                    questions.keep_draft(index, text);
                    Some(index)
                }
                Some(Answered {
                    label,
                    answer,
                    notes,
                    reply,
                }) => {
                    {
                        let output = &mut lock(&self.session.shared).output;
                        output.show_answer(&label, &answer);
                        if let Some(notes) = notes {
                            output.show_answer("notes", &notes);
                        }
                    }
                    if let Some((ask, answer)) = reply {
                        self.reply(&ask, answer)?;
                    }
                    next_shown(questions, index)
                }
            },
            Outcome::Questions(text) => {
                questions.keep_draft(index, text);
                None
            }
            Outcome::Next(text) => {
                questions.keep_draft(index, text);
                Some((index + 1) % count)
            }
            Outcome::Previous(text) => {
                questions.keep_draft(index, text);
                Some((index + count - 1) % count)
            }
            // Claude Code asked something more, which the next round of the loop adds
            Outcome::Break(text) => {
                questions.keep_draft(index, text);
                Some(index)
            }
            // Claude Code takes or refuses a call's questions together
            Outcome::Stop(_) => {
                let ask = questions.remove_call(index);
                self.reply(&ask, Answer::Deny)?;
                next_shown(questions, index)
            }
            Outcome::End => unreachable!("the session ends before"),
        })
    }

    /// Refuses what Claude asks, and closes Claude Code's input, as the end of input does a
    /// program's in the terminal, since the user has nothing more to say.
    fn end_input(mut self, questions: &mut Questions) -> Result<Option<Ending<'a>>, String> {
        for ask in questions.remove_all() {
            self.reply(&ask, Answer::Deny)?;
        }
        let Conversation {
            session,
            to_claude,
            claude,
            request_count: _,
        } = self;
        drop(to_claude);
        Ok(Some(Ending { session, claude }))
    }

    /// Tells Claude Code the answer to what it asked.
    fn reply(&mut self, ask: &Ask, answer: Answer) -> Result<(), String> {
        // The result of a refused tool repeats the refusal
        match answer {
            Answer::Deny => {
                let output = &mut lock(&self.session.shared).output;
                output.hide_result(&ask.tool_use_id);
                output.note_interrupt(Interruption::Stop);
            }
            Answer::DenyFor(_) => lock(&self.session.shared)
                .output
                .hide_result(&ask.tool_use_id),
            Answer::Allow { .. } => {}
        }
        self.send(&Command::Answer(ask, answer))
    }

    /// Shows the message and sends it. While Claude works, Claude Code makes way for it, as for
    /// a message sent with its own send-now key.
    fn send_message(&mut self, text: &str) -> Result<(), String> {
        let is_working = lock(&self.session.shared).output.status() != Status::Idle;
        lock(&self.session.shared).output.show_message(text);
        let id = Uuid::new_v4().to_string();
        self.send(&Command::Message { id: &id, text })?;
        if is_working {
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
        lock(&self.session.shared)
            .output
            .show_error(&format!("Refused {subtype}: not supported"));
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
                Err(format!("Cannot write to claude: {error}"))
            }
            _ => Ok(()),
        }
    }
}

/// The session once the user has nothing more to say: Claude Code's input closed, and Claude
/// finishes its work.
pub struct Ending<'a> {
    session: Session,
    /// Claude Code's process, which Ctrl-C interrupts.
    claude: &'a Child,
}

impl Ending<'_> {
    /// Shows Claude finish its work, until Claude Code exits once its turn and its background
    /// tasks end. Ctrl-C stops them.
    pub fn run(mut self) -> Result<(), String> {
        loop {
            match self.session.next_input() {
                Some(Input::Closed) => return self.session.close_line(),
                // Undocumented, seen with Claude Code 2.1.316: once its input closed, Claude Code
                // fails each permission request, with "Stream closed" as the tool's result
                Some(Input::Ask(_) | Input::Unsupported { .. }) | None => {}
            }
            match self.session.input.read(Phase::Ending, "")? {
                Outcome::Stop(_) => self.interrupt()?,
                // Claude Code fails what it asks once its input closed, and the ending takes no
                // other key
                Outcome::Break(_)
                | Outcome::Send(_)
                | Outcome::Questions(_)
                | Outcome::Next(_)
                | Outcome::Previous(_)
                | Outcome::End => {}
            }
        }
    }

    /// Undocumented, seen with Claude Code 2.1.316: SIGINT stops Claude's turn and background
    /// tasks, as Ctrl-C does a program's work in the terminal, and Claude Code then exits, since
    /// its input closed.
    fn interrupt(&self) -> Result<(), String> {
        lock(&self.session.shared)
            .output
            .note_interrupt(Interruption::Stop);
        // SAFETY: kill only sends a signal. The id still names Claude Code's process: the system
        // keeps a child's id until its parent waits for it, and waiting needs exclusive access
        // to the child, which this borrow of it rules out
        if unsafe { libc::kill(self.claude.id().cast_signed(), libc::SIGINT) } == -1 {
            return Err(format!(
                "Cannot stop claude: {}",
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

/// The question the question view shows once the one at `index` is answered or refused: the
/// one that took its place, or the new last one, or none once none waits, which takes the user
/// back to the message.
fn next_shown(questions: &Questions, index: usize) -> Option<usize> {
    (!questions.is_empty()).then(|| index.min(questions.count() - 1))
}
