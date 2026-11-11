//! A question's options and the user's own answer, as the question view shows them, and what the
//! keys do to them. The options are numbered from 1, and below them an item marked `>` stands for
//! the user's own answer, which the user writes in the input box. The keys that pick an option
//! pick that item too, which takes the cursor into the box. There, Enter finishes the answer, or
//! on a question of several options ticks it as one more, and Esc in normal mode leaves the box.
//!
//! `]` and `[` move to the next and previous question, as they move between tabs in lazygit.
//! Ref: <https://coreyt.github.io/monospace-design-tui/standard/keyboard/>, on `[`/`]` for tabs

use std::collections::BTreeSet;

use crossterm::event::{KeyCode, KeyEvent};

use crate::render::{DIM, GREEN, LIGHT_CYAN, RESET, markdown_lines};

/// The key of the user's own answer, and its mark, the same on every question.
pub const OWN_KEY: char = '>';

/// The most options Claude Code allows on a question, as of Claude Code 2.1.321, which checks a
/// question's options against `.min(2).max(4)`. The options take as many rows on every question,
/// so the input area keeps its size from one question to the next.
const OPTION_ROWS: u16 = 4;

/// The rows the options of a question of `choices` options take. More options than
/// `OPTION_ROWS` take more rows only so that none of them is hidden: they mean Claude Code changed
/// its limit, and `OPTION_ROWS` should change with it.
pub fn option_rows(choices: usize) -> u16 {
    u16::try_from(choices).unwrap_or(u16::MAX).max(OPTION_ROWS)
}

/// An option, with the lines that show above the question while the cursor is on it.
#[derive(Clone)]
pub struct Choice {
    pub label: String,
    pub description: String,
    pub preview: Option<String>,
}

/// The form of the answer a question takes.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Form {
    /// Takes one option, or the user's own answer.
    One,
    /// Takes any options together, and the user's own answer as one more.
    Several,
    /// The review of a batch's answers, one option each, ticked to send and unticked to edit.
    /// It has no item of the user's own.
    Review,
}

/// Where the cursor is.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Cursor {
    /// On the option at this index, from 0.
    Option(usize),
    /// On the item of the user's own answer.
    Own,
    /// In the input box, where the user writes their own answer.
    Writing,
}

/// What the user has picked so far on a question.
#[derive(Clone, PartialEq, Debug)]
pub struct Pick {
    pub cursor: Cursor,
    /// The options ticked, on a question of several options.
    pub ticked: BTreeSet<usize>,
    /// Whether the user's own answer is ticked, on a question of several options.
    pub is_own_ticked: bool,
}

/// What a question keeps until it is answered: the pick, and the text of the user's own answer.
#[derive(Clone, PartialEq, Debug)]
pub struct Draft {
    pub pick: Pick,
    pub text: String,
}

/// What a key does besides changing the pick.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Effect {
    /// Nothing more.
    Stay,
    /// The input box takes the key, as a vi key.
    Type,
    /// The cursor goes into the input box, at the end of the text, in insert mode.
    StartWriting,
    /// The cursor leaves the input box, which goes to normal mode.
    StopWriting,
    /// The user answers with what is picked.
    Submit,
    /// The user goes back to the message.
    Back,
    /// The user moves to the next question.
    Next,
    /// The user moves to the previous question.
    Previous,
}

/// What the keys depend on in the input box.
#[derive(Clone, Copy)]
pub struct InputBox {
    /// Whether the box is in normal mode with no command half typed, where Esc, `]` and `[`
    /// leave it instead of acting as vi keys.
    pub is_idle: bool,
    pub is_empty: bool,
}

impl Draft {
    /// A question's draft before the user picks anything: the cursor on the first option.
    pub fn new() -> Self {
        Draft {
            pick: Pick {
                cursor: Cursor::Option(0),
                ticked: BTreeSet::new(),
                is_own_ticked: false,
            },
            text: String::new(),
        }
    }
}

impl Pick {
    /// Acts on `key`, Enter among them, on a question of `choices` options that takes `form`,
    /// with `input` the state of the box.
    pub fn press(&mut self, key: &KeyEvent, choices: usize, form: Form, input: InputBox) -> Effect {
        match self.cursor {
            Cursor::Writing => self.press_in_box(key, form, input),
            Cursor::Option(_) | Cursor::Own => self.press_on_list(key, choices, form),
        }
    }

    /// Moves the cursor one item down the list of `choices` options of `form`, from the box too.
    pub fn move_down(&mut self, choices: usize, form: Form) -> Effect {
        let effect = self.leave_box();
        self.cursor = match self.cursor {
            Cursor::Option(index) if index + 1 < choices => Cursor::Option(index + 1),
            Cursor::Option(index) if form == Form::Review => Cursor::Option(index),
            _ => Cursor::Own,
        };
        effect
    }

    /// Moves the cursor one item up the list of `choices` options, from the box too.
    pub fn move_up(&mut self, choices: usize) -> Effect {
        let effect = self.leave_box();
        self.cursor = match self.cursor {
            Cursor::Option(index) => Cursor::Option(index.saturating_sub(1)),
            _ => Cursor::Option(choices - 1),
        };
        effect
    }

    /// Takes the cursor into the box, as pasting does.
    pub fn start_writing(&mut self) -> Effect {
        self.cursor = Cursor::Writing;
        Effect::StartWriting
    }

    /// Whether answering now leaves out the user's own text: an option picked over it, or on a
    /// question of several options, the user's own answer left unticked.
    pub fn leaves_out(&self, form: Form, text: &str) -> bool {
        let is_left_out = match form {
            Form::One => matches!(self.cursor, Cursor::Option(_)),
            Form::Several => !self.is_own_ticked,
            Form::Review => false,
        };
        is_left_out && !text.trim().is_empty()
    }

    /// What chosen together on a question that takes `form` with `text` as the user's own: the
    /// options and the user's own answer. One option takes the option the cursor is on, or else
    /// the user's own answer. Several take the options ticked, and the user's own answer when it
    /// is ticked, or with neither, the option the cursor is on.
    pub fn chosen(&self, form: Form, text: &str) -> Chosen {
        let focused = match self.cursor {
            Cursor::Option(index) => Some(index),
            Cursor::Own | Cursor::Writing => None,
        };
        let has_text = !text.trim().is_empty();
        if form != Form::Several {
            return Chosen {
                options: focused.into_iter().collect(),
                is_own: focused.is_none() && has_text,
            };
        }
        let is_own = self.is_own_ticked && has_text;
        let mut options: Vec<usize> = self.ticked.iter().copied().collect();
        if options.is_empty() && !is_own {
            options.extend(focused);
        }
        Chosen { options, is_own }
    }

    fn press_on_list(&mut self, key: &KeyEvent, choices: usize, form: Form) -> Effect {
        let ticks = form != Form::One;
        match (key.code, self.cursor, form) {
            (KeyCode::Down | KeyCode::Char('j'), _, _) => self.move_down(choices, form),
            (KeyCode::Up | KeyCode::Char('k'), _, _) => self.move_up(choices),
            (KeyCode::Char(OWN_KEY), _, Form::Review) => Effect::Stay,
            (KeyCode::Enter, Cursor::Own, Form::One) | (KeyCode::Char(OWN_KEY), _, _) => {
                self.start_writing()
            }
            (KeyCode::Enter, _, _) => Effect::Submit,
            (KeyCode::Char(' '), Cursor::Option(index), _) if ticks => {
                if !self.ticked.remove(&index) {
                    self.ticked.insert(index);
                }
                Effect::Stay
            }
            (KeyCode::Char(' '), Cursor::Own, Form::Several) if self.is_own_ticked => {
                self.is_own_ticked = false;
                Effect::Stay
            }
            (KeyCode::Char(' '), Cursor::Own, Form::Several) => self.start_writing(),
            (KeyCode::Char(digit @ '1'..='9'), _, _) => {
                let index = usize::from(digit as u8 - b'1');
                if index >= choices {
                    return Effect::Stay;
                }
                self.cursor = Cursor::Option(index);
                if !ticks {
                    return Effect::Submit;
                }
                if !self.ticked.remove(&index) {
                    self.ticked.insert(index);
                }
                Effect::Stay
            }
            (KeyCode::Esc, _, _) => Effect::Back,
            (KeyCode::Char(']'), _, _) => Effect::Next,
            (KeyCode::Char('['), _, _) => Effect::Previous,
            _ => Effect::Stay,
        }
    }

    fn press_in_box(&mut self, key: &KeyEvent, form: Form, input: InputBox) -> Effect {
        match (key.code, form) {
            (KeyCode::Enter, Form::One) => Effect::Submit,
            // On a question of several options, the text is one more item, which Enter ticks
            (KeyCode::Enter, Form::Several) => {
                self.is_own_ticked = !input.is_empty;
                self.leave_box()
            }
            (KeyCode::Esc, _) if input.is_idle => self.leave_box(),
            (KeyCode::Char(']'), _) if input.is_idle => Effect::Next,
            (KeyCode::Char('['), _) if input.is_idle => Effect::Previous,
            _ => Effect::Type,
        }
    }

    /// Acts on `key` on a submitted question, whose answer stays as it is until Enter opens it.
    pub fn press_submitted(key: &KeyEvent) -> Effect {
        match key.code {
            KeyCode::Enter => Effect::Submit,
            KeyCode::Esc => Effect::Back,
            KeyCode::Char(']') => Effect::Next,
            KeyCode::Char('[') => Effect::Previous,
            _ => Effect::Stay,
        }
    }

    /// Takes the cursor out of the box, onto the item of the user's own, when it is in the box.
    fn leave_box(&mut self) -> Effect {
        if self.cursor != Cursor::Writing {
            return Effect::Stay;
        }
        self.cursor = Cursor::Own;
        Effect::StopWriting
    }
}

/// The options and the user's own answer that a question's answer holds.
pub struct Chosen {
    pub options: Vec<usize>,
    pub is_own: bool,
}

/// The lines of `choices`, and of the item of the user's own answer, which says `own_label`. The
/// pointer marks the item the cursor is on, dimmed while the cursor is in the box, and each item's
/// key takes the width of a number's, `1.`, so the items line up. A submitted question shows its
/// answer, `submitted`, in green, marked as a tick marks an option, and dims the rest.
pub fn lines(
    choices: &[Choice],
    own_label: &str,
    form: Form,
    pick: &Pick,
    submitted: Option<&Chosen>,
) -> (Vec<String>, String) {
    let start = |key: &str, cursor: Cursor, is_ticked: bool| {
        let (pointer, color, is_ticked) = match submitted {
            Some(chosen) => {
                let is_chosen = match cursor {
                    Cursor::Option(index) => chosen.options.contains(&index),
                    Cursor::Own | Cursor::Writing => chosen.is_own,
                };
                match (is_chosen, form) {
                    (true, Form::One) => (format!("{GREEN}✓{RESET}"), GREEN, false),
                    (true, _) => (" ".to_string(), GREEN, true),
                    (false, _) => (" ".to_string(), DIM, false),
                }
            }
            None => match (pick.cursor == cursor, pick.cursor) {
                (true, _) => (format!("{LIGHT_CYAN}❯{RESET}"), LIGHT_CYAN, is_ticked),
                (false, Cursor::Writing) if cursor == Cursor::Own => {
                    (format!("{DIM}❯{RESET}"), "", is_ticked)
                }
                (false, _) => (" ".to_string(), "", is_ticked),
            },
        };
        let tick = match (form, is_ticked) {
            (Form::One, _) => "",
            (_, true) => "[✓] ",
            (_, false) => "[ ] ",
        };
        format!("{pointer} {color}{key:<2} {tick}")
    };
    let options = choices
        .iter()
        .enumerate()
        .map(|(index, choice)| {
            let start = start(
                &format!("{}.", index + 1),
                Cursor::Option(index),
                pick.ticked.contains(&index),
            );
            let description = if choice.description.is_empty() {
                String::new()
            } else {
                format!("  {DIM}{}", choice.description)
            };
            format!("{start}{}{RESET}{description}", choice.label)
        })
        .collect();
    if form == Form::Review {
        return (options, String::new());
    }
    let own = start(&OWN_KEY.to_string(), Cursor::Own, pick.is_own_ticked);
    (options, format!("{own}{own_label}{RESET}"))
}

/// The preview of the option the cursor is on.
pub struct Preview {
    /// The option as its line shows it, like `1. Postgres`, none off the options.
    pub title: Option<String>,
    /// None when the option has no preview.
    pub lines: Vec<String>,
}

/// The preview of the option the cursor is on, or none when no option of the question has a
/// preview.
pub fn preview(choices: &[Choice], pick: &Pick) -> Option<Preview> {
    if choices.iter().all(|choice| choice.preview.is_none()) {
        return None;
    }
    let focused = match pick.cursor {
        Cursor::Option(index) => Some((index, &choices[index])),
        Cursor::Own | Cursor::Writing => None,
    };
    Some(Preview {
        title: focused.map(|(index, choice)| format!("{}. {}", index + 1, choice.label)),
        // Claude Code shows a preview as Markdown, which Claude then writes it in
        lines: markdown_lines(
            focused
                .and_then(|(_, choice)| choice.preview.as_deref())
                .unwrap_or_default(),
        ),
    })
}

/// What the label row says the keys do, after the question's header, with `pick` on `choices`
/// options. On the review, Enter sends the answers when all are ticked, and else opens the
/// unticked ones, which the label says, so Enter never surprises.
pub fn hint(form: Form, pick: &Pick, choices: usize) -> String {
    let unticked = choices - pick.ticked.len();
    match (form, pick.cursor == Cursor::Writing) {
        (Form::Review, _) if unticked == 0 => "space unticks to edit, enter sends".to_string(),
        (Form::Review, _) => format!("enter edits {unticked}"),
        (Form::One, false) => "enter submits".to_string(),
        (Form::Several, false) => "space ticks, enter submits".to_string(),
        (Form::One, true) => "enter submits, esc goes back".to_string(),
        (Form::Several, true) => "enter adds it, esc goes back".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crossterm::event::KeyModifiers;

    const IDLE: InputBox = InputBox {
        is_idle: true,
        is_empty: false,
    };

    fn pick(cursor: Cursor) -> Pick {
        Pick {
            cursor,
            ..Draft::new().pick
        }
    }

    fn press(pick: &mut Pick, form: Form, keys: &str) -> Vec<Effect> {
        keys.chars()
            .map(|character| {
                let key = KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE);
                pick.press(&key, 3, form, IDLE)
            })
            .collect()
    }

    fn press_code(pick: &mut Pick, form: Form, code: KeyCode, input: InputBox) -> Effect {
        pick.press(&KeyEvent::new(code, KeyModifiers::NONE), 3, form, input)
    }

    #[test]
    fn j_and_k_move_between_the_options_and_the_users_own() {
        let mut pick = pick(Cursor::Option(0));
        press(&mut pick, Form::One, "k");
        assert_eq!(pick.cursor, Cursor::Option(0));
        press(&mut pick, Form::One, "jjjj");
        assert_eq!(pick.cursor, Cursor::Own);
        press(&mut pick, Form::One, "k");
        assert_eq!(pick.cursor, Cursor::Option(2));
    }

    #[test]
    fn a_number_picks_its_option_or_ticks_it() {
        let mut pick = pick(Cursor::Option(0));
        assert_eq!(press(&mut pick, Form::One, "2"), [Effect::Submit]);
        assert_eq!(pick.cursor, Cursor::Option(1));
        assert_eq!(press(&mut pick, Form::One, "9"), [Effect::Stay]);
        assert_eq!(press(&mut pick, Form::Several, "3 "), [Effect::Stay; 2]);
        assert!(pick.ticked.is_empty());
        press(&mut pick, Form::Several, "1");
        assert_eq!(pick.ticked, BTreeSet::from([0]));
    }

    #[test]
    fn the_keys_that_pick_an_option_take_the_users_own_into_the_box() {
        let mut one = pick(Cursor::Own);
        assert_eq!(
            press_code(&mut one, Form::One, KeyCode::Enter, IDLE),
            Effect::StartWriting
        );
        assert_eq!(one.cursor, Cursor::Writing);
        let mut several = pick(Cursor::Own);
        assert_eq!(
            press_code(&mut several, Form::Several, KeyCode::Enter, IDLE),
            Effect::Submit
        );
        assert_eq!(
            press(&mut several, Form::Several, " "),
            [Effect::StartWriting]
        );
        let mut anywhere = pick(Cursor::Option(1));
        assert_eq!(press(&mut anywhere, Form::One, ">"), [Effect::StartWriting]);
    }

    #[test]
    fn in_the_box_enter_finishes_and_esc_leaves_only_when_idle() {
        let mut one = pick(Cursor::Writing);
        assert_eq!(
            press_code(&mut one, Form::One, KeyCode::Enter, IDLE),
            Effect::Submit
        );
        let mut several = pick(Cursor::Writing);
        assert_eq!(
            press_code(&mut several, Form::Several, KeyCode::Enter, IDLE),
            Effect::StopWriting
        );
        assert!(several.is_own_ticked);
        assert_eq!(several.cursor, Cursor::Own);
        let typing = InputBox {
            is_idle: false,
            is_empty: false,
        };
        let mut esc = pick(Cursor::Writing);
        assert_eq!(
            press_code(&mut esc, Form::Several, KeyCode::Esc, typing),
            Effect::Type
        );
        assert_eq!(
            press_code(&mut esc, Form::Several, KeyCode::Esc, IDLE),
            Effect::StopWriting
        );
        assert!(!esc.is_own_ticked);
        let mut digit = pick(Cursor::Writing);
        assert_eq!(press(&mut digit, Form::One, "2"), [Effect::Type]);
    }

    #[test]
    fn space_unticks_the_users_own_and_what_is_left_out_is_told() {
        let mut pick = Pick {
            cursor: Cursor::Own,
            ticked: BTreeSet::new(),
            is_own_ticked: true,
        };
        assert!(!pick.leaves_out(Form::Several, "fuzzing"));
        assert_eq!(press(&mut pick, Form::Several, " "), [Effect::Stay]);
        assert!(!pick.is_own_ticked);
        assert!(pick.leaves_out(Form::Several, "fuzzing"));
        assert!(!pick.leaves_out(Form::Several, " "));
        assert!(!pick.leaves_out(Form::One, "fuzzing"));
        pick.cursor = Cursor::Option(0);
        assert!(pick.leaves_out(Form::One, "fuzzing"));
    }

    #[test]
    fn ctrl_n_and_ctrl_p_leave_the_box() {
        let mut pick = pick(Cursor::Writing);
        assert_eq!(pick.move_up(3), Effect::StopWriting);
        assert_eq!(pick.cursor, Cursor::Option(2));
    }

    #[test]
    fn the_review_ticks_its_answers_and_has_no_own() {
        let mut pick = pick(Cursor::Option(0));
        pick.ticked = BTreeSet::from([0, 1, 2]);
        assert_eq!(press(&mut pick, Form::Review, "jjj>"), [Effect::Stay; 4]);
        assert_eq!(pick.cursor, Cursor::Option(2));
        press(&mut pick, Form::Review, " 1");
        assert_eq!(pick.ticked, BTreeSet::from([1]));
        assert_eq!(hint(Form::Review, &pick, 3), "enter edits 2");
    }

    #[test]
    fn a_submitted_question_takes_only_enter_esc_and_brackets() {
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        assert_eq!(Pick::press_submitted(&key(KeyCode::Enter)), Effect::Submit);
        assert_eq!(Pick::press_submitted(&key(KeyCode::Esc)), Effect::Back);
        assert_eq!(
            Pick::press_submitted(&key(KeyCode::Char(']'))),
            Effect::Next
        );
        for code in [KeyCode::Char('j'), KeyCode::Char('1'), KeyCode::Char(' ')] {
            assert_eq!(Pick::press_submitted(&key(code)), Effect::Stay);
        }
    }

    #[test]
    fn esc_goes_back_and_brackets_move_between_questions() {
        let mut pick = pick(Cursor::Option(0));
        assert_eq!(
            press_code(&mut pick, Form::One, KeyCode::Esc, IDLE),
            Effect::Back
        );
        assert_eq!(
            press(&mut pick, Form::One, "]["),
            [Effect::Next, Effect::Previous]
        );
    }

    #[test]
    fn a_question_with_any_preview_shows_the_preview_of_each_option() {
        let choice = |preview: Option<&str>| Choice {
            label: String::new(),
            description: String::new(),
            preview: preview.map(str::to_string),
        };
        let choices = [choice(Some("a\nb\nc")), choice(None)];
        let first = preview(&choices, &pick(Cursor::Option(0))).expect("a preview");
        assert_eq!(first.title.as_deref(), Some("1. "));
        assert_eq!(first.lines.len(), 3);
        let second = preview(&choices, &pick(Cursor::Option(1))).expect("a preview");
        assert_eq!(second.lines, Vec::<String>::new());
        let own = preview(&choices, &pick(Cursor::Own)).expect("a preview");
        assert!(own.title.is_none());
        assert!(preview(&choices[1..], &pick(Cursor::Option(0))).is_none());
    }

    #[test]
    fn more_options_than_claude_code_allows_take_more_rows() {
        assert_eq!(option_rows(2), OPTION_ROWS);
        assert_eq!(option_rows(6), 6);
    }
}
