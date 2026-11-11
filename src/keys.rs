//! What each key does. Keys come in two kinds. The session keys, Ctrl with a letter, act on the
//! session the same in every vi mode, so the key alone decides which one it is. Every other key
//! works on the text: Enter sends it, and the rest edit it with the vi keys of the input box. In
//! the question view, they also move between the options above the box.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::picker::{Choice, Form};

/// What the user does on the input line.
#[derive(Clone)]
pub enum Phase {
    /// Writes a message to Claude, while `waiting` questions of Claude's wait. A question waits
    /// for Ctrl-Y rather than taking the input box as it arrives, since a key typed just then
    /// would answer it. Claude Code's own interface instead puts a question in place of the
    /// input box, where typing only answers it, and refusing it ends the turn before the user
    /// writes again. So Enter on a message refuses the waiting questions, which makes it a press
    /// that `loss` names.
    ///
    /// Claude Code 2.1.321 also accepts a plain message while a question waits, though its
    /// interface never sends one: the message queues behind the question, and Claude reads it
    /// right after the answer, in the same turn. A message sent with send now cancels the
    /// question instead.
    Writing { waiting: usize },
    /// Answers the question the question view shows.
    Answering(View),
    /// Has nothing more to say, while Claude finishes its work, which only Ctrl-C stops.
    Ending,
}

impl Phase {
    /// Whether the input line takes text, typed or pasted.
    pub fn takes_text(&self) -> bool {
        !matches!(self, Phase::Ending)
    }
}

/// What the question view shows: one of the questions that wait.
#[derive(Clone)]
pub struct View {
    /// The question's header, which the label row shows.
    pub label: String,
    /// The question's text, or the plan, above the options.
    pub lines: Vec<String>,
    /// The question's options, or a plan's approval and refusal.
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
}

/// What a session key does. None of them changes the text.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum SessionKey {
    /// Ctrl-C: stops Claude's turn, refusing what Claude asks.
    Stop,
    /// Ctrl-D: ends the session, refusing what Claude asks.
    End,
    /// Ctrl-L: clears the context, refusing what Claude asks and stopping Claude's turn, and
    /// keeps the text, which can be the new context's first message. Ctrl-L clears the screen
    /// in a shell, and starts a new session here, whose start a line marks in the conversation.
    Clear,
    /// Ctrl-O: shows the whole conversation, from the question the question view shows.
    ShowFullView,
    /// Ctrl-S: shows the status.
    ShowStatus,
    /// Ctrl-Y: opens the questions that wait, or goes back to the message from them, as Esc
    /// also does on the list of options.
    Questions,
    /// Ctrl-N: moves down the options of the question the question view shows, to the user's own
    /// answer after the last, from the input box too, as Ctrl-N moves down a list in fzf and in
    /// vim's completion menu.
    Down,
    /// Ctrl-P: moves up the options, like `Down`.
    Up,
}

impl SessionKey {
    /// The session key that `key` is, from the key alone.
    fn of(key: &KeyEvent) -> Option<SessionKey> {
        if !key.modifiers.contains(KeyModifiers::CONTROL) {
            return None;
        }
        match key.code {
            KeyCode::Char('c') => Some(SessionKey::Stop),
            KeyCode::Char('d') => Some(SessionKey::End),
            KeyCode::Char('l') => Some(SessionKey::Clear),
            KeyCode::Char('o') => Some(SessionKey::ShowFullView),
            KeyCode::Char('s') => Some(SessionKey::ShowStatus),
            KeyCode::Char('y') => Some(SessionKey::Questions),
            KeyCode::Char('n') => Some(SessionKey::Down),
            KeyCode::Char('p') => Some(SessionKey::Up),
            _ => None,
        }
    }
}

/// What the session holds that a press can lose.
#[derive(Clone, Copy)]
pub struct Stakes {
    /// Whether the input has text, which Enter sends.
    pub has_text: bool,
    pub held: Held,
    /// Whether Claude works on a turn.
    pub is_working: bool,
}

/// What the context holds, which a clear loses, and whether the prompt cache still keeps it.
#[derive(Clone, Copy, PartialEq)]
pub enum Held {
    Nothing,
    Warm,
    /// The cache is cold, so the next call writes the whole context again, at a price above
    /// that of new input.
    Cold,
}

/// What `action` loses in `phase` that no key brings back, with `stakes` telling what the
/// session holds. Such a press waits for the same key again in a row, which the input line carries out
/// and the label row names the loss for, as Codex and Claude Code do for their exit keys. The
/// label names the loss, since a confirmation guards only when it says what is about to happen,
/// and it shows only for a loss, so the second press never becomes a habit. Stopping Claude's
/// turn takes one press, as in every agent's interface: a message tells Claude to go on, and a
/// stop delayed costs more than one made by mistake.
/// Ref: <https://www.saasui.design/blog/saas-destructive-actions-confirmation-ux-patterns>, on
/// a confirmation's text naming the outcome
pub fn loss(phase: &Phase, action: Action, stakes: Stakes) -> Option<String> {
    match (phase, action) {
        (Phase::Writing { .. } | Phase::Answering(_), Action::Session(SessionKey::End)) => {
            Some("ends the session".to_string())
        }
        (Phase::Writing { waiting }, Action::Session(SessionKey::Stop)) if *waiting > 0 => {
            Some(format!("refuses {}", questions(*waiting)))
        }
        // The question view refuses the call it shows, with each of its questions
        (Phase::Answering(view), Action::Session(SessionKey::Stop)) => {
            Some(format!("refuses {}", questions(view.progress.len())))
        }
        // Claude Code stops its background tasks too, and exits
        (Phase::Ending, Action::Session(SessionKey::Stop)) => Some("stops claude".to_string()),
        (Phase::Writing { waiting }, Action::Send) if stakes.has_text => {
            let refusal = (*waiting > 0).then(|| format!("refuses {} and ", questions(*waiting)));
            let cache = (stakes.held == Held::Cold).then_some(" on the cold cache");
            (refusal.is_some() || cache.is_some()).then(|| {
                format!(
                    "{}sends{}",
                    refusal.unwrap_or_default(),
                    cache.unwrap_or_default()
                )
            })
        }
        // The session goes on in a new one, but the clear loses the cache, and with questions
        // waiting or Claude working, what the turn would have done. Refusing a question stops
        // the turn too
        (Phase::Writing { waiting }, Action::Session(SessionKey::Clear))
            if stakes.held != Held::Nothing =>
        {
            let turn = if *waiting > 0 {
                format!("refuses {} and ", questions(*waiting))
            } else if stakes.is_working {
                "stops claude and ".to_string()
            } else {
                String::new()
            };
            Some(format!("{turn}clears the context"))
        }
        _ => None,
    }
}

fn questions(count: usize) -> String {
    if count == 1 {
        "the question".to_string()
    } else {
        format!("{count} questions")
    }
}

/// The name of `key` on the label row, as the user knows it from the keyboard.
pub fn name(key: &KeyEvent) -> String {
    let base = match key.code {
        KeyCode::Enter => "enter".to_string(),
        KeyCode::Char(' ') => "space".to_string(),
        KeyCode::Char(character) => character.to_string(),
        code => code.to_string().to_lowercase(),
    };
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        format!("ctrl-{base}")
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
    /// Edits the text with the input box's vi keys, or in the question view, acts on the options
    /// or the input box.
    Edit,
    Ignore,
}

/// What `key` does in `phase`, with `is_empty` telling whether the input has no text. Enter and
/// the session keys do their job even while a vi command is half typed, since they are not vi
/// keys. Ctrl-D ends only an empty input, as in a shell, so a draft is never lost to it.
/// Ref: <https://www.gnu.org/software/bash/manual/html_node/Commands-For-Text.html>, on Ctrl-D
/// as the end of input only on an empty line
pub fn action(phase: &Phase, key: &KeyEvent, is_empty: bool) -> Action {
    match (phase, SessionKey::of(key)) {
        (Phase::Ending, Some(SessionKey::Stop)) => Action::Session(SessionKey::Stop),
        (Phase::Ending, _)
        | (Phase::Answering(_), Some(SessionKey::Clear))
        | (Phase::Writing { waiting: 0, .. }, Some(SessionKey::Questions))
        | (Phase::Writing { .. }, Some(SessionKey::Down | SessionKey::Up)) => Action::Ignore,
        (_, Some(SessionKey::End)) if !is_empty => Action::Ignore,
        (_, Some(session_key)) => Action::Session(session_key),
        (_, None) if key.code == KeyCode::Enter => Action::Send,
        (_, None) => Action::Edit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn control(character: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(character), KeyModifiers::CONTROL)
    }

    fn plain(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn writing(waiting: usize) -> Phase {
        Phase::Writing { waiting }
    }

    fn answering() -> Phase {
        Phase::Answering(View {
            label: "plan".to_string(),
            lines: vec![],
            choices: vec![],
            own_label: String::new(),
            form: Form::One,
            full_view_line: 1,
            index: 0,
            progress: vec![false, false],
            is_submitted: false,
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
                (
                    control('o'),
                    false,
                    Action::Session(SessionKey::ShowFullView),
                ),
                (control('s'), false, Action::Session(SessionKey::ShowStatus)),
                (control('w'), false, Action::Edit),
                (plain(KeyCode::Enter), false, Action::Send),
                (plain(KeyCode::Char('r')), false, Action::Edit),
                (plain(KeyCode::Esc), false, Action::Edit),
            ];
            for (key, is_empty, expected) in cases {
                assert_eq!(action(&phase, &key, is_empty), expected);
            }
        }
    }

    #[test]
    fn ctrl_y_opens_only_questions_that_wait_and_enter_still_sends() {
        let none = writing(0);
        assert_eq!(action(&none, &control('y'), false), Action::Ignore);
        let waiting = writing(2);
        let cases = [
            (control('y'), Action::Session(SessionKey::Questions)),
            (plain(KeyCode::Enter), Action::Send),
            (control('n'), Action::Ignore),
            (control('p'), Action::Ignore),
            (plain(KeyCode::Char('y')), Action::Edit),
        ];
        for (key, expected) in cases {
            assert_eq!(action(&waiting, &key, false), expected);
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
            assert_eq!(action(&answering(), &key, false), expected);
        }
    }

    const WARM: Stakes = Stakes {
        has_text: true,
        held: Held::Warm,
        is_working: false,
    };

    const COLD: Stakes = Stakes {
        held: Held::Cold,
        ..WARM
    };

    fn assert_losses(action: Action, cases: Vec<(Phase, Stakes, Option<&str>)>) {
        for (phase, stakes, expected) in cases {
            assert_eq!(loss(&phase, action, stakes).as_deref(), expected);
        }
    }

    #[test]
    fn ctrl_c_and_ctrl_d_name_what_they_end_and_a_stopped_turn_takes_one_press() {
        let end = Action::Session(SessionKey::End);
        assert_losses(
            end,
            vec![
                (writing(0), WARM, Some("ends the session")),
                (answering(), WARM, Some("ends the session")),
            ],
        );
        let stop = Action::Session(SessionKey::Stop);
        let working = Stakes {
            is_working: true,
            ..WARM
        };
        assert_losses(
            stop,
            vec![
                (writing(1), WARM, Some("refuses the question")),
                (writing(3), WARM, Some("refuses 3 questions")),
                (answering(), WARM, Some("refuses 2 questions")),
                (Phase::Ending, WARM, Some("stops claude")),
                (writing(0), working, None),
            ],
        );
        let questions = Action::Session(SessionKey::Questions);
        assert_losses(questions, vec![(writing(2), WARM, None)]);
        assert_losses(Action::Edit, vec![(writing(0), COLD, None)]);
    }

    #[test]
    fn a_message_names_the_questions_it_refuses_and_the_cold_cache() {
        let empty = Stakes {
            has_text: false,
            ..COLD
        };
        assert_losses(
            Action::Send,
            vec![
                (writing(2), WARM, Some("refuses 2 questions and sends")),
                (writing(0), COLD, Some("sends on the cold cache")),
                (
                    writing(1),
                    COLD,
                    Some("refuses the question and sends on the cold cache"),
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
            has_text: false,
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
    fn keys_go_by_their_keyboard_names() {
        assert_eq!(name(&control('c')), "ctrl-c");
        assert_eq!(name(&plain(KeyCode::Enter)), "enter");
        assert_eq!(name(&plain(KeyCode::Char(' '))), "space");
        assert_eq!(name(&plain(KeyCode::Char('2'))), "2");
    }

    #[test]
    fn ctrl_l_clears_from_the_message_with_text_too() {
        for is_empty in [false, true] {
            assert_eq!(
                action(&writing(2), &control('l'), is_empty),
                Action::Session(SessionKey::Clear)
            );
        }
        assert_eq!(action(&answering(), &control('l'), true), Action::Ignore);
    }

    #[test]
    fn the_ending_takes_only_ctrl_c_which_stops() {
        assert_eq!(
            action(&Phase::Ending, &control('c'), true),
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
            assert_eq!(action(&Phase::Ending, &key, true), Action::Ignore);
        }
    }
}
