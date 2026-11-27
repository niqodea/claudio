//! What each key does. Keys come in two kinds. The session keys, Ctrl with a letter, act on the
//! session the same in every vi mode, so the key alone decides which one it is. Every other key
//! works on the text: Enter sends it, and the rest edit it with the vi keys of the input box. In
//! the question view, they also move between the options above the box.

use termina::event::{KeyCode, KeyEvent, Modifiers};

use crate::context::Held;
use crate::picker::{Choice, Form};

/// What the user does on the input line: one of the readings of keys, each with the effects
/// its keys can have on the session.
#[derive(Clone)]
pub enum Phase {
    Writing(Writing),
    Answering(Answering),
    Ending(Ending),
}

/// Writes a message to Claude, while `waiting` questions of Claude's wait. A question waits for
/// Ctrl+Y rather than taking the input box as it arrives, since a key typed just then would
/// answer it. Claude Code's own interface instead puts a question in place of the input box,
/// where typing only answers it, and refusing it ends the turn before the user writes again. So
/// Enter on a message refuses the waiting questions, which makes it a press that names its loss.
///
/// Claude Code also accepts a plain message while a question waits, though its interface
/// never sends one: the message queues behind the question, and Claude reads it right after the
/// answer, in the same turn. A message sent with send now cancels the question instead.
#[derive(Clone)]
pub struct Writing {
    pub waiting: usize,
}

/// Answers the question the question view shows.
#[derive(Clone)]
pub struct Answering {
    pub view: Box<View>,
}

/// Has nothing more to say, while Claude finishes its work, which Ctrl+C and Ctrl+K stop.
#[derive(Clone)]
pub struct Ending;

/// A phase's reading of keys, which decides what they do to the session. Each phase has its own
/// kind of effect, so the session gets only the effects of the phase it read keys in.
pub trait Reading: Clone {
    type Effect: Effect;

    /// The phase, as the label row shows it.
    fn phase(&self) -> Phase;

    /// What `action` does to the session, with `stakes` telling what the session holds, or none
    /// for an action that acts on the input line alone, or that has nothing to act on, like
    /// Enter on an empty box. Every action has an arm of its own, so a new one has to state
    /// what it does to the session.
    fn effect(&self, action: Action, stakes: Stakes) -> Option<Self::Effect>;
}

impl Phase {
    /// Whether the input line takes text, typed or pasted.
    pub fn takes_text(&self) -> bool {
        !matches!(self, Phase::Ending(_))
    }

    /// What `action` loses, with `stakes` telling what the session holds, as its effect says.
    pub fn loss(&self, action: Action, stakes: Stakes) -> Option<String> {
        match self {
            Phase::Writing(reading) => reading.effect(action, stakes)?.loss(),
            Phase::Answering(reading) => reading.effect(action, stakes)?.loss(),
            Phase::Ending(reading) => reading.effect(action, stakes)?.loss(),
        }
    }
}

/// The rows of the block under the title, the same on every question and call, so that the title
/// and the options stay in place from one to the next. A short screen gives it fewer rows, the
/// same on every question and call too.
pub const BLOCK_ROWS: usize = 8;

/// The rows of the block that a question whose options have previews keeps for its text. The
/// preview takes the rows after them, so that the options stay in place on every question, with
/// or without previews.
pub const PREVIEWED_QUESTION_ROWS: usize = 3;

/// The block's last rows, which a permission request keeps for what it says about itself rather
/// than about the call: why Claude Code asks, after `?`, and who asked, after `↳`. Each is always
/// there, as a grey note when Claude Code says nothing, so these rows read the same on every
/// request, and the call's fields above them take `INPUT_ROWS` rows on every call of a tool.
pub const ABOUT_ROWS: usize = 2;

/// The rows of the block a permission request gives the call's fields, above `ABOUT_ROWS`.
pub const INPUT_ROWS: usize = BLOCK_ROWS - ABOUT_ROWS;

/// A part of the block under the title, which takes `rows` rows: a call's field, a line about a
/// permission request, a question's text, or an option's preview.
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    /// What shows in front of the value.
    pub mark: Mark,
    /// The value, with its line breaks.
    pub value: String,
    /// Whether the value is a file's path or a URL, which a row too narrow for it cuts in the
    /// middle, where any other value is cut at its end.
    pub is_path: bool,
    pub tone: Tone,
    pub rows: usize,
}

/// What shows in front of a field's value, in a column of the same width on every call of a
/// tool, so that its values never move sideways.
#[derive(Clone, Debug, PartialEq)]
pub enum Mark {
    /// Nothing, as in front of a question's text.
    None,
    /// A sign that reads as what the value is, like `$` in front of a command, in a column of
    /// two, in front of the value's first row, or of each of its rows on `every_row`, as a diff
    /// marks its lines. Its tone is the field's, whatever the value: dim, or a diff's red or
    /// green.
    Sign {
        sign: char,
        every_row: bool,
        tone: Tone,
    },
    /// The field's name, cut to a column of the same width on every tool, in front of a value of
    /// one row.
    Name(String),
    /// A preview of an option, whose value is the preview's Markdown, which shows highlighted, as
    /// `picker::preview_lines` gives its lines, each on its own row behind a bar in the margin.
    /// The option's key, like `2▎`, goes in front of the first row, or nothing when the cursor is
    /// on no option. The bar, rather than a title row, keeps the preview a value like any other,
    /// which gives up rows down to 1 and keeps its sign at its end.
    /// Ref: <https://docs.rs/crate/synaps-tui/0.3.3/source/src/tui/render.rs>, which sets its tool
    /// blocks apart with a bar in their margin
    Quote(Option<String>),
}

/// How a field's value shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tone {
    Plain,
    /// A question's text.
    Bold,
    /// claudio's note in place of a value, like `not given`, dim as all that stays in the
    /// background.
    Note,
    /// Claude's own words on a call, like the description of a command, dim as a comment.
    Comment,
    /// The text an edit takes out, in red, as in a diff.
    Removed,
    /// The text an edit puts in, in green, as in a diff.
    Added,
}

/// What the question view shows: one of the questions that wait, in one of the requests that
/// wait, a call's questions, a plan or a permission.
#[derive(Clone)]
pub struct View {
    /// Which one of its kind the request is, which the label row shows: a question's header, a
    /// call's tool, or `Plan`.
    pub label: String,
    /// The request's kind, `Question`, `Plan` or `Permission`, which the title above the block
    /// shows, with the request's place when more than one waits: the label row names the
    /// request, and the block shows the rest.
    pub title: String,
    /// What Ctrl+C refuses: the request shown, like `the call` or `2 questions`.
    pub refusal: String,
    /// Which request it is, from 0.
    pub request_index: usize,
    /// Whether each request that waits has all its questions submitted.
    pub requests: Vec<bool>,
    /// What the block of `BLOCK_ROWS` rows under the title shows: a question's text, a plan, or
    /// a call's fields and reason.
    pub fields: Vec<Field>,
    /// The question's options, or the approval and the refusal of a call or a plan.
    pub choices: Vec<Choice>,
    /// What the item of the user's own answer says.
    pub own_label: String,
    pub form: Form,
    /// The line of the full view, from 1, that the question starts on.
    pub full_view_line: usize,
    /// Which question of its batch it is, from 0, where one past the last is the review.
    pub index: usize,
    /// Whether each question of the batch is submitted.
    pub progress: Vec<bool>,
    /// Whether the question is submitted, which keeps its answer until Enter opens it again.
    pub is_submitted: bool,
    /// Whether Enter sends the answer at once, as on a batch of one question, which has nothing
    /// to review.
    pub sends_at_once: bool,
}

/// What a session key does. Only `Edit` changes the text, through the user's editor.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum SessionKey {
    /// Ctrl+C: stops Claude's turn, refusing what Claude asks.
    Stop,
    /// Ctrl+K: stops everything that runs: Claude's turn, as `Stop` does, and every task in the
    /// background, as `k` kills a process in htop and Ctrl+K a pod in k9s.
    Kill,
    /// Ctrl+D: ends the session, refusing what Claude asks.
    End,
    /// Ctrl+L: clears the context, refusing what Claude asks and stopping Claude's turn, and
    /// keeps the text, which can be the new context's first message. Ctrl+L clears the screen
    /// in a shell, and starts a new session here, whose start a line marks in the conversation.
    Clear,
    /// Ctrl+E: opens the text in the user's editor, as readline's Ctrl+X Ctrl+E does, and takes
    /// back what it saved.
    Edit,
    /// Ctrl+O: shows the whole conversation, from the question the question view shows.
    ShowFullView,
    /// Ctrl+S: shows the status.
    ShowStatus,
    /// Ctrl+Y: opens the questions that wait, or goes back to the message from them, as Esc
    /// also does on the list of options.
    Questions,
    /// Ctrl+N: moves down the options of the question the question view shows, to the user's own
    /// answer after the last, from the input box too, as Ctrl+N moves down a list in fzf and in
    /// vim's completion menu.
    Down,
    /// Ctrl+P: moves up the options, like `Down`.
    Up,
}

impl SessionKey {
    /// The session key that `key` is, from the key alone.
    fn of(key: &KeyEvent) -> Option<SessionKey> {
        if !key.modifiers.contains(Modifiers::CONTROL) {
            return None;
        }
        match key.code {
            KeyCode::Char('c') => Some(SessionKey::Stop),
            KeyCode::Char('k') => Some(SessionKey::Kill),
            KeyCode::Char('d') => Some(SessionKey::End),
            KeyCode::Char('l') => Some(SessionKey::Clear),
            KeyCode::Char('e') => Some(SessionKey::Edit),
            KeyCode::Char('o') => Some(SessionKey::ShowFullView),
            KeyCode::Char('s') => Some(SessionKey::ShowStatus),
            KeyCode::Char('y') => Some(SessionKey::Questions),
            KeyCode::Char('n') => Some(SessionKey::Down),
            KeyCode::Char('p') => Some(SessionKey::Up),
            _ => None,
        }
    }
}

/// What the keys depend on in the input line.
#[derive(Clone, Copy)]
pub struct Input {
    /// Whether the box is in normal mode with no command half typed, where some keys leave it
    /// instead of acting as vi keys: `:` on a message, and Esc, `]` and `[` in the question view,
    /// whose own keys stay vi keys there.
    pub is_idle: bool,
    /// Whether the box holds nothing but spaces.
    pub is_empty: bool,
    /// Whether the command line is open, which takes the keys that edit, in the box's place.
    pub is_commanding: bool,
}

/// What the session holds that a press can lose.
#[derive(Clone, Copy)]
pub struct Stakes<'a> {
    /// What Enter sends or runs: the box's text, or the command line's while open.
    pub text: &'a str,
    pub held: Held,
    /// Whether Claude works on a turn.
    pub is_working: bool,
    /// The ids of the tasks that run in the background.
    pub background: &'a [String],
    /// Whether a clear the user asked for has not reached the new session yet.
    pub is_clear_pending: bool,
}

impl Stakes<'_> {
    /// What a clear or the end waits for.
    fn wait(self) -> Wait {
        Wait::of(self.is_working, !self.background.is_empty())
    }
}

/// What a key does to the session, decided once, from the state the key is judged against. It
/// holds all that the decision took from the state, and the session carries it out as decided,
/// without judging the state again, so the loss the label row names and what happens come from
/// one value. Each phase has its own kind of effect, so the session gets only the effects that
/// the phase it read keys in can have.
pub trait Effect {
    /// What the effect loses that no key brings back. Such a press waits for the same key again
    /// in a row, which `Gate` carries out and the label row names the loss for, as Codex and
    /// Claude Code do for their exit keys. The label names the loss, since a confirmation guards
    /// only when it says what is about to happen, and it shows only for a loss, so the second
    /// press never becomes a habit. Stopping Claude's turn takes one press, as in every agent's
    /// interface: a message tells Claude to go on, and a stop delayed costs more than one made
    /// by mistake.
    /// Ref: <https://www.saasui.design/blog/saas-destructive-actions-confirmation-ux-patterns>,
    /// on a confirmation's text naming the outcome
    fn loss(&self) -> Option<String>;
}

/// What a key does while the user writes a message.
#[derive(Clone, Debug, PartialEq)]
pub enum OnMessage {
    /// Sends the box's text as a message.
    Send(Sending),
    /// Runs the command line's command of Claude Code's, written without its `/`, as `Send`
    /// sends a message.
    Run(Sending),
    /// Stops Claude's turn, with one press when it refuses no question.
    Stop(TurnStop),
    /// Stops everything that runs: the turn that runs, if any, as `stop` says, and the tasks
    /// that run in the background, by their ids, never none, since with none it is a `Stop`.
    Kill {
        stop: Option<TurnStop>,
        tasks: Vec<String>,
    },
    /// Forces the pending clear: stops the turn that runs, if any, as `stop` says, and the
    /// tasks in the background that the clear waits for, by their ids, after which it clears.
    ForceClear {
        stop: Option<TurnStop>,
        tasks: Vec<String>,
    },
    /// Clears the context, which holds something, after stopping the turn that runs, if any.
    /// Whether it waits for the tasks in the background is decided as it goes, rather than with
    /// the key, under the lock that notes the tasks, so the end of the last one never slips past
    /// it.
    Clear(Option<TurnStop>),
    /// Ends the session's input, after which the session waits for what `Wait` names.
    End(Wait),
}

/// What a key does while the user answers the question the question view shows.
#[derive(Clone, Debug, PartialEq)]
pub enum OnQuestion {
    /// Submits the question view's pick: answers the question, opens a submitted one again, or
    /// sends the review's answers. An answer's loss is its choice's `risk`, as the choice names
    /// it, or else the user's own text it leaves out, when `leaves_out_own`.
    Submit {
        risk: Option<String>,
        leaves_out_own: bool,
    },
    /// Refuses the request the question view shows, which `refusal` names, like `the call`.
    Refuse { refusal: String },
    /// Ends the session's input, after which the session waits for what `Wait` names.
    End(Wait),
}

/// What a clear or the end waits for before it goes through, which the label row names. The
/// tasks in the background come first, since a task may never end, like a server, while a turn
/// does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Wait {
    Nothing,
    Turn,
    BackgroundTasks,
}

impl Wait {
    /// The wait while Claude `is_working` on a turn and while tasks run in the background, as
    /// `has_background_tasks` says.
    pub fn of(is_working: bool, has_background_tasks: bool) -> Self {
        if has_background_tasks {
            Wait::BackgroundTasks
        } else if is_working {
            Wait::Turn
        } else {
            Wait::Nothing
        }
    }

    /// Whether the wait may never end without the user's key, as for a task like a server,
    /// while Claude's turn ends by itself.
    pub fn may_never_end(self) -> bool {
        self == Wait::BackgroundTasks
    }

    /// What the session waits for, like `Claude's turn`, none for `Nothing`.
    pub fn work(self) -> Option<&'static str> {
        match self {
            Wait::Nothing => None,
            Wait::Turn => Some("Claude's turn"),
            Wait::BackgroundTasks => Some("background tasks"),
        }
    }

    /// The loss of the end, which names the wait, so the user knows that the session goes on
    /// after the second press.
    fn loss(self) -> String {
        match self.work() {
            Some(work) => format!("ends the session after {work}"),
            None => "ends the session".to_string(),
        }
    }
}

/// What a key does once the user has nothing more to say, while Claude finishes its work. Each
/// stops Claude's turn and its background tasks, after which Claude Code exits.
#[derive(Clone, Debug, PartialEq)]
pub enum OnEnding {
    /// Ctrl+C, which stops what the end waits for, as the label row names it.
    Stop,
    /// Ctrl+K, which stops everything, `tasks` background tasks among it, and names them as it
    /// does while the session goes on, with the turn that runs, if any, as `stop` says.
    Kill {
        stop: Option<TurnStop>,
        tasks: usize,
    },
}

/// What a message or a command sends, never empty, and what the state made it lose.
#[derive(Clone, Debug, PartialEq)]
pub struct Sending {
    pub text: String,
    /// The questions that wait, which Claude Code cancels for what the user sends, so the
    /// session refuses them first.
    pub refused: usize,
    /// Whether the cache may be cold, so that the call pays for the whole context again.
    pub is_cold: bool,
}

/// How Claude's turn stops: by refusing the questions it waits on, which ends it, or with an
/// interrupt.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TurnStop {
    Refuse(usize),
    Interrupt,
}

impl Effect for OnMessage {
    fn loss(&self) -> Option<String> {
        match self {
            // A command may call the API as a message does: some do, like /compact, and Claude
            // Code hands one it does not know to Claude, so a typo becomes a call
            OnMessage::Send(sending) => sending.loss("sends"),
            OnMessage::Run(sending) => sending.loss("runs the command"),
            OnMessage::Stop(TurnStop::Refuse(count)) => {
                Some(format!("refuses {}", questions(*count)))
            }
            // A forced clear's label row names what it stops before the press, and a pending
            // clear leaves no question waiting
            OnMessage::Stop(TurnStop::Interrupt) | OnMessage::ForceClear { .. } => None,
            OnMessage::Kill { stop, tasks } => Some(kill_loss(*stop, tasks.len())),
            // The session goes on in a new one, but the clear loses the cache, and what the turn
            // that runs would have done
            OnMessage::Clear(stop) => {
                let stop = match stop {
                    Some(TurnStop::Refuse(count)) => format!("refuses {} and ", questions(*count)),
                    Some(TurnStop::Interrupt) => "stops claude and ".to_string(),
                    None => String::new(),
                };
                Some(format!("{stop}clears the context"))
            }
            OnMessage::End(wait) => Some(wait.loss()),
        }
    }
}

impl Effect for OnQuestion {
    fn loss(&self) -> Option<String> {
        match self {
            OnQuestion::Submit {
                risk,
                leaves_out_own,
            } => risk
                .clone()
                .or_else(|| leaves_out_own.then(|| "leaves out your own answer".to_string())),
            // The question view refuses the request it shows, with each of its questions
            OnQuestion::Refuse { refusal } => Some(format!("refuses {refusal}")),
            OnQuestion::End(wait) => Some(wait.loss()),
        }
    }
}

impl Effect for OnEnding {
    fn loss(&self) -> Option<String> {
        match self {
            // The label row names what the press stops
            OnEnding::Stop => None,
            OnEnding::Kill { stop, tasks } => Some(kill_loss(*stop, *tasks)),
        }
    }
}

/// The loss of stopping `tasks` background tasks, after the turn that runs, if any, as `stop`
/// says, like `stops claude and 2 background tasks`. A task that ends in the background leaves
/// its work undone, which no message brings back the way it brings back a turn.
fn kill_loss(stop: Option<TurnStop>, tasks: usize) -> String {
    let stop = match stop {
        Some(TurnStop::Refuse(count)) => format!("refuses {} and stops ", questions(count)),
        Some(TurnStop::Interrupt) => "stops claude and ".to_string(),
        None => "stops ".to_string(),
    };
    let tasks = match tasks {
        1 => "1 background task".to_string(),
        count => format!("{count} background tasks"),
    };
    format!("{stop}{tasks}")
}

impl Sending {
    /// The loss of what `verb` sends, like `refuses 2 questions and sends on a possibly cold
    /// cache`.
    fn loss(&self, verb: &str) -> Option<String> {
        if self.refused == 0 && !self.is_cold {
            return None;
        }
        let refusal = if self.refused > 0 {
            format!("refuses {} and ", questions(self.refused))
        } else {
            String::new()
        };
        let cache = if self.is_cold {
            " on a possibly cold cache"
        } else {
            ""
        };
        Some(format!("{refusal}{verb}{cache}"))
    }
}

impl Reading for Writing {
    type Effect = OnMessage;

    fn phase(&self) -> Phase {
        Phase::Writing(self.clone())
    }

    fn effect(&self, action: Action, stakes: Stakes) -> Option<OnMessage> {
        // A turn that waits on questions stops as they are refused
        let turn_stop = match (self.waiting, stakes.is_working) {
            (0, false) => None,
            (0, true) => Some(TurnStop::Interrupt),
            (count, _) => Some(TurnStop::Refuse(count)),
        };
        // The box keeps its text for the session the clear starts, as a clear always does
        let sending =
            (!stakes.text.trim().is_empty() && !stakes.is_clear_pending).then(|| Sending {
                text: stakes.text.to_string(),
                refused: self.waiting,
                is_cold: stakes.held == Held::Cold,
            });
        match action {
            Action::Session(SessionKey::Stop) if stakes.is_clear_pending => {
                Some(OnMessage::ForceClear {
                    stop: turn_stop,
                    tasks: stakes.background.to_vec(),
                })
            }
            Action::Session(SessionKey::Stop) => turn_stop.map(OnMessage::Stop),
            Action::Session(SessionKey::Kill) if stakes.background.is_empty() => {
                turn_stop.map(OnMessage::Stop)
            }
            Action::Session(SessionKey::Kill) => Some(OnMessage::Kill {
                stop: turn_stop,
                tasks: stakes.background.to_vec(),
            }),
            Action::Session(SessionKey::End) => Some(OnMessage::End(stakes.wait())),
            // A context that holds nothing yet has nothing to clear, and one that a clear waits for
            // clears then
            Action::Session(SessionKey::Clear) => (stakes.held != Held::Nothing
                && !stakes.is_clear_pending)
                .then_some(OnMessage::Clear(turn_stop)),
            Action::Send => sending.map(OnMessage::Send),
            Action::Run => sending.map(OnMessage::Run),
            Action::Session(
                SessionKey::Edit
                | SessionKey::ShowFullView
                | SessionKey::ShowStatus
                | SessionKey::Questions
                | SessionKey::Down
                | SessionKey::Up,
            )
            | Action::CommandLine
            | Action::Complete { .. }
            | Action::Edit
            | Action::Ignore => None,
        }
    }
}

impl Reading for Answering {
    type Effect = OnQuestion;

    fn phase(&self) -> Phase {
        Phase::Answering(self.clone())
    }

    /// The answer comes from the pick instead, as `OnQuestion::Submit`, since Enter and the keys
    /// that edit and move act through it.
    fn effect(&self, action: Action, stakes: Stakes) -> Option<OnQuestion> {
        match action {
            Action::Session(SessionKey::Stop) => Some(OnQuestion::Refuse {
                refusal: self.view.refusal.clone(),
            }),
            Action::Session(SessionKey::End) => Some(OnQuestion::End(stakes.wait())),
            // The question view shows one request, which Ctrl+C refuses, and the message's line
            // shows the rest of what runs, which Ctrl+K stops there
            Action::Session(
                SessionKey::Kill
                | SessionKey::Clear
                | SessionKey::Edit
                | SessionKey::ShowFullView
                | SessionKey::ShowStatus
                | SessionKey::Questions
                | SessionKey::Down
                | SessionKey::Up,
            )
            | Action::Send
            | Action::Run
            | Action::CommandLine
            | Action::Complete { .. }
            | Action::Edit
            | Action::Ignore => None,
        }
    }
}

impl Reading for Ending {
    type Effect = OnEnding;

    fn phase(&self) -> Phase {
        Phase::Ending(self.clone())
    }

    /// Only Ctrl+C and Ctrl+K act, both stopping everything, since stopping the turn alone would
    /// leave Claude Code waiting on its background tasks still.
    fn effect(&self, action: Action, stakes: Stakes) -> Option<OnEnding> {
        match action {
            Action::Session(SessionKey::Stop) => Some(OnEnding::Stop),
            Action::Session(SessionKey::Kill) if stakes.background.is_empty() => {
                Some(OnEnding::Stop)
            }
            Action::Session(SessionKey::Kill) => Some(OnEnding::Kill {
                stop: stakes.is_working.then_some(TurnStop::Interrupt),
                tasks: stakes.background.len(),
            }),
            Action::Session(
                SessionKey::End
                | SessionKey::Clear
                | SessionKey::Edit
                | SessionKey::ShowFullView
                | SessionKey::ShowStatus
                | SessionKey::Questions
                | SessionKey::Down
                | SessionKey::Up,
            )
            | Action::Send
            | Action::Run
            | Action::CommandLine
            | Action::Complete { .. }
            | Action::Edit
            | Action::Ignore => None,
        }
    }
}

/// An effect that goes ahead: one that loses nothing, or one whose loss the label row named, let
/// through by the second press in a row of its key. Only `Gate` makes one, and the session
/// carries out only these, so no effect skips the confirmation.
#[derive(Debug)]
pub struct Confirmed<E>(E);

impl<E> Confirmed<E> {
    pub fn effect(&self) -> &E {
        &self.0
    }
}

/// The confirmation of the keys that act on the session: a key whose effect loses something
/// waits for its second press in a row, which the label row names the loss for meanwhile.
#[derive(Default)]
pub struct Gate {
    armed: Option<Armed>,
}

/// A press whose effect loses something, which waits for the same key again with the same loss.
struct Armed {
    key: KeyEvent,
    loss: String,
    /// The key's action, whose loss the session's state can take away meanwhile, as when Claude
    /// Code tells the cache is new. None for an answer's key, whose loss only keys change.
    action: Option<Action>,
}

impl Gate {
    /// Lets `effect` of the press of `key`, of `action`, go ahead, or none while it waits for
    /// the key's second press. Every key ends the wait of the one before. An effect goes ahead
    /// at once when it loses nothing, and else on the second press in a row of its key, with the
    /// same loss: a loss that changed between the presses waits for a press more, so the user
    /// always saw what a key loses before it goes ahead.
    pub fn pass<E: Effect>(
        &mut self,
        key: &KeyEvent,
        action: Option<Action>,
        effect: E,
    ) -> Option<Confirmed<E>> {
        let before = self.armed.take();
        let Some(loss) = effect.loss() else {
            return Some(Confirmed(effect));
        };
        let is_second = before.is_some_and(|first| {
            first.key.code == key.code && first.key.modifiers == key.modifiers && first.loss == loss
        });
        if is_second {
            return Some(Confirmed(effect));
        }
        self.armed = Some(Armed {
            key: *key,
            loss,
            action,
        });
        None
    }

    /// Ends the wait, as a key with no effect on the session does, and a paste or a new reading.
    pub fn reset(&mut self) {
        self.armed = None;
    }

    /// Ends the wait of a key whose loss went away in `phase`, with `stakes`, so the label row
    /// never asks for a press that loses nothing. A loss that changed instead waits for the
    /// press, which names it.
    pub fn drop_lost(&mut self, phase: &Phase, stakes: Stakes) {
        let Some(action) = self.armed.as_ref().and_then(|armed| armed.action) else {
            return;
        };
        if phase.loss(action, stakes).is_none() {
            self.armed = None;
        }
    }

    /// The key that waits and what its second press loses, like `Ctrl+L again clears the
    /// context`.
    pub fn label(&self) -> Option<String> {
        (self.armed.as_ref()).map(|armed| format!("{} again {}", name(&armed.key), armed.loss))
    }
}

pub fn questions(count: usize) -> String {
    if count == 1 {
        "the question".to_string()
    } else {
        format!("{count} questions")
    }
}

/// The name of `key` on the label row, as its keycap shows it, like Enter and Ctrl+C. A vi key
/// stays the character it types, like `j`.
pub fn name(key: &KeyEvent) -> String {
    let base = match key.code {
        KeyCode::Char(' ') => "Space".to_string(),
        KeyCode::Char(character) => character.to_string(),
        KeyCode::Escape => "Esc".to_string(),
        // termina's name for the key, like Enter
        code => format!("{code:?}"),
    };
    if key.modifiers.contains(Modifiers::CONTROL) {
        format!("Ctrl+{}", base.to_uppercase())
    } else {
        base
    }
}

/// What a key does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    Session(SessionKey),
    /// Sends the text, as a message or an answer.
    Send,
    /// Opens the command line, for one of Claude Code's commands, as `:` opens vim's.
    CommandLine,
    /// Runs the command on the command line.
    Run,
    /// Completes the command's name on the command line, with the next of the commands it
    /// starts, or with the one before for `is_backward`, as Tab and Shift+Tab do on vim's.
    Complete {
        is_backward: bool,
    },
    /// Edits the text with the input box's vi keys, or in the question view, acts on the options
    /// or the input box.
    Edit,
    Ignore,
}

/// What `key` does in `phase`, on `input`. Enter and the session keys do their job even while a
/// vi command is half typed, since they are not vi keys. `:` on a message opens the command
/// line from the idle box, as from vim's normal mode, Enter runs it, and Tab and Shift+Tab
/// complete the command's name. Ctrl+D ends only an
/// empty input, as in a shell, so a draft is never lost to it.
/// Ref: <https://www.gnu.org/software/bash/manual/html_node/Commands-For-Text.html>, on Ctrl+D
/// as the end of input only on an empty line
pub fn action(phase: &Phase, key: &KeyEvent, input: Input) -> Action {
    match (phase, SessionKey::of(key)) {
        (Phase::Ending(_), Some(session_key @ (SessionKey::Stop | SessionKey::Kill))) => {
            Action::Session(session_key)
        }
        (Phase::Ending(_), _)
        | (Phase::Answering(_), Some(SessionKey::Clear))
        | (Phase::Writing(Writing { waiting: 0 }), Some(SessionKey::Questions))
        | (Phase::Writing(_), Some(SessionKey::Down | SessionKey::Up)) => Action::Ignore,
        (_, Some(SessionKey::End)) if !input.is_empty => Action::Ignore,
        (_, Some(session_key)) => Action::Session(session_key),
        (Phase::Writing(_), None) if key.code == KeyCode::Enter && input.is_commanding => {
            Action::Run
        }
        (Phase::Writing(_), None)
            if input.is_commanding && matches!(key.code, KeyCode::Tab | KeyCode::BackTab) =>
        {
            Action::Complete {
                is_backward: key.code == KeyCode::BackTab,
            }
        }
        (_, None) if key.code == KeyCode::Enter => Action::Send,
        (Phase::Writing(_), None)
            if key.code == KeyCode::Char(':') && input.is_idle && !input.is_commanding =>
        {
            Action::CommandLine
        }
        (_, None) => Action::Edit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The input of a box in insert mode, empty or not.
    fn input(is_empty: bool) -> Input {
        Input {
            is_idle: false,
            is_empty,
            is_commanding: false,
        }
    }

    fn control(character: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(character), Modifiers::CONTROL)
    }

    fn plain(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, Modifiers::NONE)
    }

    fn writing(waiting: usize) -> Phase {
        Phase::Writing(Writing { waiting })
    }

    fn answering() -> Phase {
        Phase::Answering(Answering {
            view: Box::new(View {
                label: "plan".to_string(),
                title: "Question".to_string(),
                refusal: "2 questions".to_string(),
                request_index: 0,
                requests: vec![false],
                fields: vec![],
                choices: vec![],
                own_label: String::new(),
                form: Form::One,
                full_view_line: 1,
                index: 0,
                progress: vec![false, false],
                is_submitted: false,
                sends_at_once: false,
            }),
        })
    }

    #[test]
    fn writing_and_answering_take_the_same_keys() {
        for phase in [writing(0), answering()] {
            let cases = [
                (control('c'), false, Action::Session(SessionKey::Stop)),
                (control('c'), true, Action::Session(SessionKey::Stop)),
                (control('d'), false, Action::Ignore),
                (control('d'), true, Action::Session(SessionKey::End)),
                (control('e'), false, Action::Session(SessionKey::Edit)),
                (
                    control('o'),
                    false,
                    Action::Session(SessionKey::ShowFullView),
                ),
                (control('s'), false, Action::Session(SessionKey::ShowStatus)),
                (control('w'), false, Action::Edit),
                (plain(KeyCode::Enter), false, Action::Send),
                (plain(KeyCode::Char('r')), false, Action::Edit),
                (plain(KeyCode::Escape), false, Action::Edit),
            ];
            for (key, is_empty, expected) in cases {
                assert_eq!(action(&phase, &key, input(is_empty)), expected);
            }
        }
    }

    #[test]
    fn colon_opens_the_command_line_from_the_idle_box_of_a_message_enter_runs_it_and_tab_completes()
    {
        let idle = Input {
            is_idle: true,
            ..input(false)
        };
        let colon = plain(KeyCode::Char(':'));
        assert_eq!(action(&writing(0), &colon, idle), Action::CommandLine);
        // A colon typed in insert mode, or as part of a vi command like `f:`, is text
        assert_eq!(action(&writing(0), &colon, input(false)), Action::Edit);
        assert_eq!(action(&answering(), &colon, idle), Action::Edit);
        let commanding = Input {
            is_commanding: true,
            ..idle
        };
        assert_eq!(action(&writing(0), &colon, commanding), Action::Edit);
        let enter = plain(KeyCode::Enter);
        assert_eq!(action(&writing(0), &enter, commanding), Action::Run);
        let tab = plain(KeyCode::Tab);
        let back_tab = KeyEvent::new(KeyCode::BackTab, Modifiers::SHIFT);
        let complete = |is_backward| Action::Complete { is_backward };
        assert_eq!(action(&writing(0), &tab, commanding), complete(false));
        assert_eq!(action(&writing(0), &back_tab, commanding), complete(true));
        // Tab on the box, with the command line closed, is the box's
        assert_eq!(action(&writing(0), &tab, idle), Action::Edit);
    }

    #[test]
    fn a_command_names_the_questions_it_refuses_and_the_cold_cache() {
        let cold = Stakes {
            held: Held::Cold,
            ..WARM
        };
        assert_eq!(writing(0).loss(Action::Run, WARM), None);
        assert_eq!(
            writing(2).loss(Action::Run, WARM).as_deref(),
            Some("refuses 2 questions and runs the command")
        );
        assert_eq!(
            writing(0).loss(Action::Run, cold).as_deref(),
            Some("runs the command on a possibly cold cache")
        );
        assert_eq!(
            writing(2).loss(Action::Run, cold).as_deref(),
            Some("refuses 2 questions and runs the command on a possibly cold cache")
        );
    }

    #[test]
    fn ctrl_y_opens_only_questions_that_wait_and_enter_still_sends() {
        let none = writing(0);
        assert_eq!(action(&none, &control('y'), input(false)), Action::Ignore);
        let waiting = writing(2);
        let cases = [
            (control('y'), Action::Session(SessionKey::Questions)),
            (plain(KeyCode::Enter), Action::Send),
            (control('n'), Action::Ignore),
            (control('p'), Action::Ignore),
            (plain(KeyCode::Char('y')), Action::Edit),
        ];
        for (key, expected) in cases {
            assert_eq!(action(&waiting, &key, input(false)), expected);
        }
    }

    #[test]
    fn the_question_view_moves_between_questions_and_goes_back_with_ctrl_y() {
        let cases = [
            (control('y'), Action::Session(SessionKey::Questions)),
            (control('n'), Action::Session(SessionKey::Down)),
            (control('p'), Action::Session(SessionKey::Up)),
        ];
        for (key, expected) in cases {
            assert_eq!(action(&answering(), &key, input(false)), expected);
        }
    }

    const WARM: Stakes = Stakes {
        text: "hi",
        held: Held::Warm,
        is_working: false,
        background: &[],
        is_clear_pending: false,
    };

    const COLD: Stakes = Stakes {
        held: Held::Cold,
        ..WARM
    };

    fn assert_losses(action: Action, cases: Vec<(Phase, Stakes, Option<&str>)>) {
        for (phase, stakes, expected) in cases {
            assert_eq!(phase.loss(action, stakes).as_deref(), expected);
        }
    }

    #[test]
    fn ctrl_c_and_ctrl_d_name_what_they_end_and_a_stopped_turn_takes_one_press() {
        let end = Action::Session(SessionKey::End);
        let working = Stakes {
            is_working: true,
            ..WARM
        };
        let tasks = ["a".to_string()];
        let background = Stakes {
            background: &tasks,
            ..WARM
        };
        let both = Stakes {
            is_working: true,
            ..background
        };
        assert_losses(
            end,
            vec![
                (writing(0), WARM, Some("ends the session")),
                (answering(), WARM, Some("ends the session")),
                (
                    writing(0),
                    working,
                    Some("ends the session after Claude's turn"),
                ),
                (
                    answering(),
                    working,
                    Some("ends the session after Claude's turn"),
                ),
                (
                    writing(0),
                    background,
                    Some("ends the session after background tasks"),
                ),
                (
                    writing(0),
                    both,
                    Some("ends the session after background tasks"),
                ),
            ],
        );
        let stop = Action::Session(SessionKey::Stop);
        assert_losses(
            stop,
            vec![
                (writing(1), WARM, Some("refuses the question")),
                (writing(3), WARM, Some("refuses 3 questions")),
                (answering(), WARM, Some("refuses 2 questions")),
                (Phase::Ending(Ending), WARM, None),
                (writing(0), working, None),
            ],
        );
        let questions = Action::Session(SessionKey::Questions);
        assert_losses(questions, vec![(writing(2), WARM, None)]);
        assert_losses(Action::Edit, vec![(writing(0), COLD, None)]);
    }

    #[test]
    fn a_message_names_the_questions_it_refuses_and_the_cold_cache() {
        let empty = Stakes { text: " ", ..COLD };
        assert_losses(
            Action::Send,
            vec![
                (writing(2), WARM, Some("refuses 2 questions and sends")),
                (writing(0), COLD, Some("sends on a possibly cold cache")),
                (
                    writing(1),
                    COLD,
                    Some("refuses the question and sends on a possibly cold cache"),
                ),
                (writing(0), WARM, None),
                (writing(2), empty, None),
                // The question view holds back an answer on its own
                (answering(), COLD, None),
            ],
        );
    }

    #[test]
    fn a_clear_names_the_turn_it_ends_and_an_empty_context_has_nothing_to_lose() {
        let working = Stakes {
            is_working: true,
            ..WARM
        };
        let empty = Stakes {
            text: "",
            held: Held::Nothing,
            ..WARM
        };
        assert_losses(
            Action::Session(SessionKey::Clear),
            vec![
                (writing(0), WARM, Some("clears the context")),
                (
                    writing(0),
                    working,
                    Some("stops claude and clears the context"),
                ),
                (
                    writing(2),
                    working,
                    Some("refuses 2 questions and clears the context"),
                ),
                (writing(0), empty, None),
            ],
        );
    }

    #[test]
    fn a_kill_names_the_turn_and_the_background_tasks_it_stops() {
        let tasks = ["a".to_string(), "b".to_string()];
        let background = Stakes {
            background: &tasks,
            ..WARM
        };
        let working = Stakes {
            is_working: true,
            ..background
        };
        let alone = Stakes {
            is_working: true,
            ..WARM
        };
        assert_losses(
            Action::Session(SessionKey::Kill),
            vec![
                (writing(0), background, Some("stops 2 background tasks")),
                (
                    writing(0),
                    working,
                    Some("stops claude and 2 background tasks"),
                ),
                (
                    writing(2),
                    working,
                    Some("refuses 2 questions and stops 2 background tasks"),
                ),
                // With nothing in the background, it stops the turn as Ctrl+C does
                (writing(0), alone, None),
                (writing(2), alone, Some("refuses 2 questions")),
            ],
        );
        assert_eq!(
            Writing { waiting: 0 }.effect(Action::Session(SessionKey::Kill), background),
            Some(OnMessage::Kill {
                stop: None,
                tasks: tasks.to_vec()
            })
        );
        assert_eq!(
            Writing { waiting: 0 }.effect(Action::Session(SessionKey::Kill), WARM),
            None
        );
        assert_eq!(
            action(&Phase::Ending(Ending), &control('k'), input(true)),
            Action::Session(SessionKey::Kill)
        );
    }

    #[test]
    fn a_pending_clear_keeps_the_text_takes_no_other_clear_and_ctrl_c_forces_it() {
        let pending = Stakes {
            is_clear_pending: true,
            ..WARM
        };
        for action in [
            Action::Send,
            Action::Run,
            Action::Session(SessionKey::Clear),
        ] {
            assert_eq!(Writing { waiting: 0 }.effect(action, pending), None);
        }
        let tasks = ["a".to_string()];
        let background = Stakes {
            background: &tasks,
            ..pending
        };
        let force = Writing { waiting: 0 }.effect(Action::Session(SessionKey::Stop), background);
        assert_eq!(
            force,
            Some(OnMessage::ForceClear {
                stop: None,
                tasks: tasks.to_vec(),
            })
        );
        assert_eq!(force.and_then(|effect| effect.loss()), None);
        let kill = Writing { waiting: 0 }.effect(Action::Session(SessionKey::Kill), background);
        assert_eq!(
            kill.and_then(|effect| effect.loss()).as_deref(),
            Some("stops 1 background task")
        );
    }

    #[test]
    fn the_ending_stops_with_ctrl_c_at_once_and_with_ctrl_k_as_it_does_elsewhere() {
        let tasks = ["a".to_string()];
        let working = Stakes {
            is_working: true,
            background: &tasks,
            ..WARM
        };
        let ending = |key, stakes| Ending.effect(Action::Session(key), stakes);
        assert_eq!(ending(SessionKey::Stop, working), Some(OnEnding::Stop));
        assert_eq!(ending(SessionKey::Kill, WARM), Some(OnEnding::Stop));
        assert_eq!(
            ending(SessionKey::Kill, working)
                .and_then(|effect| effect.loss())
                .as_deref(),
            Some("stops claude and 1 background task")
        );
    }

    #[test]
    fn keys_go_by_their_keyboard_names() {
        assert_eq!(name(&control('c')), "Ctrl+C");
        assert_eq!(name(&plain(KeyCode::Enter)), "Enter");
        assert_eq!(name(&plain(KeyCode::Char(' '))), "Space");
        assert_eq!(name(&plain(KeyCode::Escape)), "Esc");
        assert_eq!(name(&plain(KeyCode::Char('2'))), "2");
    }

    #[test]
    fn ctrl_l_clears_from_the_message_with_text_too() {
        for is_empty in [false, true] {
            assert_eq!(
                action(&writing(2), &control('l'), input(is_empty)),
                Action::Session(SessionKey::Clear)
            );
        }
        assert_eq!(
            action(&answering(), &control('l'), input(true)),
            Action::Ignore
        );
    }

    #[test]
    fn the_ending_takes_only_ctrl_c_which_stops() {
        assert_eq!(
            action(&Phase::Ending(Ending), &control('c'), input(true)),
            Action::Session(SessionKey::Stop)
        );
        for key in [
            control('d'),
            control('o'),
            control('s'),
            control('y'),
            control('n'),
            plain(KeyCode::Enter),
            plain(KeyCode::Char('a')),
        ] {
            assert_eq!(
                action(&Phase::Ending(Ending), &key, input(true)),
                Action::Ignore
            );
        }
    }

    /// Presses Ctrl+L at `gate`, in a session that holds `stakes`, and returns whether the clear
    /// goes ahead.
    fn press_clear(gate: &mut Gate, stakes: Stakes) -> bool {
        let clear = Action::Session(SessionKey::Clear);
        let effect = Writing { waiting: 0 }
            .effect(clear, stakes)
            .expect("a context to clear");
        gate.pass(&control('l'), Some(clear), effect).is_some()
    }

    #[test]
    fn a_loss_goes_ahead_on_the_second_press_that_the_label_row_named() {
        let mut gate = Gate::default();
        assert!(!press_clear(&mut gate, WARM));
        assert_eq!(
            gate.label().as_deref(),
            Some("Ctrl+L again clears the context")
        );
        assert!(press_clear(&mut gate, WARM));
        assert_eq!(gate.label(), None);
    }

    #[test]
    fn a_changed_loss_or_another_key_waits_for_a_press_more() {
        let working = Stakes {
            is_working: true,
            ..WARM
        };
        let mut gate = Gate::default();
        assert!(!press_clear(&mut gate, WARM));
        // Claude started working, so the second press would stop the turn too
        assert!(!press_clear(&mut gate, working));
        assert!(press_clear(&mut gate, working));
        assert!(!press_clear(&mut gate, WARM));
        // A key with no effect on the session ends the wait
        gate.reset();
        assert!(!press_clear(&mut gate, WARM));
    }

    #[test]
    fn a_key_with_nothing_to_act_on_has_no_effect() {
        let stop = Action::Session(SessionKey::Stop);
        let clear = Action::Session(SessionKey::Clear);
        let blank = Stakes { text: " ", ..WARM };
        let fresh = Stakes {
            held: Held::Nothing,
            ..WARM
        };
        assert_eq!(Writing { waiting: 0 }.effect(stop, WARM), None);
        assert_eq!(Writing { waiting: 0 }.effect(Action::Send, blank), None);
        assert_eq!(Writing { waiting: 0 }.effect(clear, fresh), None);
        assert_eq!(
            Writing { waiting: 2 }.effect(stop, WARM),
            Some(OnMessage::Stop(TurnStop::Refuse(2)))
        );
    }
}
