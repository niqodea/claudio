//! What each key does. Keys come in two kinds. The session keys, Ctrl with a letter, act on the
//! session the same in every vi mode, so the key alone decides which one it is. Every other key
//! works on the text: Enter sends it, and the rest edit it with the vi keys of the input box.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// What the user does on the input line.
#[derive(Clone)]
pub enum Phase {
    /// Writes a message to Claude, while this many questions of Claude's wait. A question waits
    /// for Ctrl-Y rather than taking the input box as it arrives, since a key typed just then
    /// would answer it. Enter sends no message while any waits, since Claude's turn waits on
    /// the answers.
    Writing(usize),
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

/// What the question view shows: one of the questions that wait, which the input box answers.
#[derive(Clone)]
pub struct View {
    /// What the label row says in front of the answer.
    pub label: String,
    /// The question in full, above the label row.
    pub lines: Vec<String>,
    /// Which question it is, from 0, of how many wait.
    pub index: usize,
    pub count: usize,
}

/// What a session key does. None of them changes the text.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum SessionKey {
    /// Ctrl-C: stops Claude's turn, refusing what Claude asks.
    Stop,
    /// Ctrl-D: ends the session, refusing what Claude asks.
    End,
    /// Ctrl-O: shows the whole conversation.
    ShowFullView,
    /// Ctrl-S: shows the status.
    ShowStatus,
    /// Ctrl-Y: opens the questions that wait, or goes back to the message from them. The same
    /// key goes back, since Esc belongs to the vi keys.
    Questions,
    /// Ctrl-N: shows the next question, as Ctrl-N shows the next line in readline.
    Next,
    /// Ctrl-P: shows the previous question, as Ctrl-P shows the previous line in readline.
    Previous,
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
            KeyCode::Char('n') => Some(SessionKey::Next),
            KeyCode::Char('p') => Some(SessionKey::Previous),
            _ => None,
        }
    }
}

/// What a key does.
#[derive(Debug, PartialEq)]
pub enum Action {
    Session(SessionKey),
    /// Sends the text, as a message or an answer.
    Send,
    /// Edits the text, with the input box's vi keys.
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
        | (Phase::Writing(0), Some(SessionKey::Questions))
        | (Phase::Writing(_), Some(SessionKey::Next | SessionKey::Previous)) => Action::Ignore,
        (_, Some(SessionKey::End)) if !is_empty => Action::Ignore,
        (_, Some(session_key)) => Action::Session(session_key),
        (Phase::Writing(waiting), None) if key.code == KeyCode::Enter && *waiting > 0 => {
            Action::Ignore
        }
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

    fn answering() -> Phase {
        Phase::Answering(View {
            label: "approve the plan? y/n".to_string(),
            lines: vec![],
            index: 0,
            count: 2,
        })
    }

    #[test]
    fn writing_and_answering_take_the_same_keys() {
        for phase in [Phase::Writing(0), answering()] {
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
    fn ctrl_y_opens_only_questions_that_wait_and_holds_the_message_meanwhile() {
        let none = Phase::Writing(0);
        assert_eq!(action(&none, &control('y'), false), Action::Ignore);
        let waiting = Phase::Writing(2);
        let cases = [
            (control('y'), Action::Session(SessionKey::Questions)),
            (plain(KeyCode::Enter), Action::Ignore),
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
            (control('n'), Action::Session(SessionKey::Next)),
            (control('p'), Action::Session(SessionKey::Previous)),
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
