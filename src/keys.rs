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
    /// writes again. So the first Enter on a message keeps it in the box and sets
    /// `refusal_confirmed`, and the second Enter refuses the waiting questions and sends it. A
    /// question that arrives in between clears `refusal_confirmed`, so Enter never refuses a
    /// question the user has not seen.
    ///
    /// Claude Code 2.1.321 also accepts a plain message while a question waits, though its
    /// interface never sends one: the message queues behind the question, and Claude reads it
    /// right after the answer, in the same turn. A message sent with send now cancels the
    /// question instead.
    Writing {
        waiting: usize,
        refusal_confirmed: bool,
    },
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
            KeyCode::Char('o') => Some(SessionKey::ShowFullView),
            KeyCode::Char('s') => Some(SessionKey::ShowStatus),
            KeyCode::Char('y') => Some(SessionKey::Questions),
            KeyCode::Char('n') => Some(SessionKey::Down),
            KeyCode::Char('p') => Some(SessionKey::Up),
            _ => None,
        }
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
        Phase::Writing {
            waiting,
            refusal_confirmed: false,
        }
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
