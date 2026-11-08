//! What each key does in each phase of the session, in one place. The keys that act on the
//! session come first in every phase, so the vi keys of the input box only ever get the rest.

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

/// What a key does.
#[derive(Debug, PartialEq)]
pub enum Action {
    /// Edits the text, with the input box's vi keys.
    Edit,
    /// Empties the input line.
    Clear,
    /// Sends the text, as a message or an answer.
    Send,
    /// Stops Claude's turn, refusing the tool that asks.
    Stop,
    /// Ends the session, refusing the tool that asks.
    End,
    ShowFullView,
    ShowStatus,
    Ignore,
}

/// What `key` does in `phase`, with `is_empty` telling whether the input line has no text, and
/// `awaits_character` whether the vi keys take the next key as a character, like after `r`,
/// which then takes a plain character or Enter. Ctrl-C empties a line with text before it stops
/// anything, and Ctrl-D ends only on an empty line, as in a shell.
/// Ref: <https://www.gnu.org/software/bash/manual/html_node/Commands-For-Text.html>, on Ctrl-D
/// as the end of input only on an empty line
pub fn action(phase: &Phase, key: &KeyEvent, is_empty: bool, awaits_character: bool) -> Action {
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    match (phase, key.code) {
        (Phase::Ending, KeyCode::Char('c')) if control => Action::Stop,
        (Phase::Ending, _) => Action::Ignore,
        (_, KeyCode::Char(_) | KeyCode::Enter) if awaits_character && !control => Action::Edit,
        (_, KeyCode::Char('c')) if control && !is_empty => Action::Clear,
        (_, KeyCode::Char('c')) if control => Action::Stop,
        (_, KeyCode::Char('d')) if control && is_empty => Action::End,
        (_, KeyCode::Char('d')) if control => Action::Ignore,
        (_, KeyCode::Char('o')) if control => Action::ShowFullView,
        (_, KeyCode::Char('s')) if control => Action::ShowStatus,
        (_, KeyCode::Enter) => Action::Send,
        _ => Action::Edit,
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
                (control('c'), false, Action::Clear),
                (control('c'), true, Action::Stop),
                (control('d'), false, Action::Ignore),
                (control('d'), true, Action::End),
                (control('o'), true, Action::ShowFullView),
                (control('s'), true, Action::ShowStatus),
                (plain(KeyCode::Enter), false, Action::Send),
                (plain(KeyCode::Char('r')), false, Action::Edit),
                (plain(KeyCode::Esc), false, Action::Edit),
            ];
            for (key, is_empty, expected) in cases {
                assert_eq!(action(&phase, &key, is_empty, false), expected);
            }
        }
    }

    #[test]
    fn the_ending_takes_only_ctrl_c_which_stops() {
        assert_eq!(
            action(&Phase::Ending, &control('c'), true, false),
            Action::Stop
        );
        for key in [
            control('d'),
            control('o'),
            control('s'),
            plain(KeyCode::Enter),
            plain(KeyCode::Char('a')),
        ] {
            assert_eq!(action(&Phase::Ending, &key, true, false), Action::Ignore);
        }
    }

    #[test]
    fn a_character_the_vi_keys_await_goes_to_them_and_the_session_keys_still_act() {
        let phase = Phase::Writing;
        assert_eq!(
            action(&phase, &plain(KeyCode::Enter), false, true),
            Action::Edit
        );
        assert_eq!(
            action(&phase, &plain(KeyCode::Char('x')), false, true),
            Action::Edit
        );
        assert_eq!(action(&phase, &control('c'), false, true), Action::Clear);
    }
}
