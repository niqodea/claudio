//! What each key does. Keys come in two kinds. The session keys, Ctrl with a letter, act on the
//! session the same in every vi mode, so the key alone decides which one it is. Every other key
//! works on the text: Enter sends it, and the rest edit it with the vi keys of the input box.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// What the user does on the input line.
#[derive(Clone)]
pub enum Phase {
    /// Writes a message to Claude.
    Writing,
    /// Answers what a tool asks, with the question.
    Answering(String),
    /// Has nothing more to say, while Claude finishes its work, which only Ctrl-C stops.
    Ending,
}

impl Phase {
    /// Whether the input line takes text, typed or pasted.
    pub fn takes_text(&self) -> bool {
        !matches!(self, Phase::Ending)
    }
}

/// What a session key does. None of them changes the text.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum SessionKey {
    /// Ctrl-C: stops Claude's turn, refusing the tool that asks.
    Stop,
    /// Ctrl-D: ends the session, refusing the tool that asks.
    End,
    /// Ctrl-O: shows the whole conversation.
    ShowFullView,
    /// Ctrl-S: shows the status.
    ShowStatus,
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
        (Phase::Ending, _) => Action::Ignore,
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

    #[test]
    fn writing_and_answering_take_the_same_keys() {
        let question = Phase::Answering("allow Bash? y/N".to_string());
        for phase in [Phase::Writing, question] {
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
    fn the_ending_takes_only_ctrl_c_which_stops() {
        assert_eq!(
            action(&Phase::Ending, &control('c'), true),
            Action::Session(SessionKey::Stop)
        );
        for key in [
            control('d'),
            control('o'),
            control('s'),
            plain(KeyCode::Enter),
            plain(KeyCode::Char('a')),
        ] {
            assert_eq!(action(&Phase::Ending, &key, true), Action::Ignore);
        }
    }
}
