//! A question's options and the user's own answer, as the question view shows them, and what the
//! keys do to them. The options are numbered from 1, and below them an item marked `>` stands for
//! the user's own answer, which the user writes in the input box. An option's number, or its
//! letter, moves the cursor to it, and only Enter answers, as AGENTS.md's rule for keys says.
//! Enter on the `>` item, or the `>` key, takes the cursor into the box. There, Enter finishes the
//! answer, or on a question of several options ticks it as one more, and Esc in normal mode
//! leaves the box.
//!
//! `]` and `[` move to the next and previous question, as they move between tabs in lazygit, and
//! the home row's `a`, `s`, `d` and `f` go straight to the first four, which letters name apart
//! from the options' numbers. `}` and `{`, and `q`, `w`, `e` and `r` on the row above, do the same
//! for the requests that wait, each a call's questions, a plan or a permission.
//! Ref: <https://coreyt.github.io/monospace-design-tui/standard/keyboard/>, on `[`/`]` for tabs

use std::collections::BTreeSet;

use crossterm::event::{KeyCode, KeyEvent};

use crate::keys::Input;
use crate::markdown::is_fence;
use crate::render::markdown_lines;
use crate::style::{ATTENTION, DONE_OR_ADDED, MARK, PLAIN, STRONG, SUBDUED};
use crate::text::{Line, Span};
use crate::wording::{Hint, hints};

/// The key of the user's own answer, and its mark, the same on every question.
pub const OWN_KEY: char = '>';

/// The keys that go to a batch's questions, which also name them. Claude Code asks at most four
/// questions in a call, as of Claude Code 2.1.322, so a fifth would have no key, and its name
/// would show as `·`.
pub const QUESTION_KEYS: [char; 4] = ['a', 's', 'd', 'f'];

/// The name of the batch's question at `index`.
pub fn question_name(index: usize) -> char {
    QUESTION_KEYS.get(index).copied().unwrap_or('·')
}

/// The keys that go to the first four requests that wait, which also name them, on the row above
/// the questions' keys, as requests hold questions. A request after the fourth has no key, and
/// `}` reaches it.
pub const REQUEST_KEYS: [char; 4] = ['q', 'w', 'e', 'r'];

/// The name of the request at `index`, none after the fourth.
pub fn request_name(index: usize) -> Option<char> {
    REQUEST_KEYS.get(index).copied()
}

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
    /// The letter that names the option and moves the cursor to it, in place of its number, like
    /// `y` and `n` for a decision, as command-line prompts answer a yes-or-no question.
    pub key: Option<char>,
    /// What choosing the option risks, like allowing a dangerous call, which the label row
    /// names while Enter waits for its second press. The option shows in the color of
    /// attention, and its label says the risk too.
    pub risk: Option<String>,
}

impl Choice {
    /// An option named by its number.
    pub fn new(label: &str, description: &str, preview: Option<String>) -> Self {
        Choice {
            label: label.to_string(),
            description: description.to_string(),
            preview,
            key: None,
            risk: None,
        }
    }
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
    /// The user moves to another question.
    Go(Move),
}

/// Where the user moves among a batch's questions, or among the requests that wait.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Move {
    Next,
    Previous,
    /// To the question at this index, from 0.
    To(usize),
    NextRequest,
    PreviousRequest,
    /// To the request at this index, from 0.
    ToRequest(usize),
}

/// Where `key` moves among the questions and the requests: `]` and `[`, `}` and `{`, and the
/// questions' and the requests' own keys.
fn move_of(key: &KeyEvent) -> Option<Move> {
    let position = |keys: &[char], name: char| keys.iter().position(|&key| key == name);
    match key.code {
        KeyCode::Char(']') => Some(Move::Next),
        KeyCode::Char('[') => Some(Move::Previous),
        KeyCode::Char('}') => Some(Move::NextRequest),
        KeyCode::Char('{') => Some(Move::PreviousRequest),
        KeyCode::Char(name) => position(&QUESTION_KEYS, name)
            .map(Move::To)
            .or_else(|| position(&REQUEST_KEYS, name).map(Move::ToRequest)),
        _ => None,
    }
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
    /// Acts on `key`, Enter among them, on a question of `choices` that takes `form`, with
    /// `input` the state of the box.
    pub fn press(
        &mut self,
        key: &KeyEvent,
        choices: &[Choice],
        form: Form,
        input: Input,
    ) -> Effect {
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

    /// What is chosen on a question that takes `form`, with `text` as the user's own: the options
    /// and the user's own answer. One option takes the option the cursor is on, or else the
    /// user's own answer. Several take exactly the options ticked, and the user's own answer when
    /// it is ticked, never the option the cursor is on, as checkboxes do: Enter sends what the
    /// ticks show.
    ///
    /// Every question needs an answer, so an empty one waits, as Clack's multiselect does by
    /// default. Claude Code 2.1.322 instead lets a question go unanswered and leaves it out of
    /// what Claude reads, so Claude cannot tell it from a choice of none. To choose none, the
    /// user writes it.
    /// Ref: <https://tessl.io/registry/tessl/npm-clack--prompts/0.11.0/files/docs/selection-prompts.md>,
    /// on Clack's multiselect taking `required`, true by default
    /// Ref: <https://github.com/anthropics/claude-code/issues/30320>, on asking Claude Code to
    /// tell Claude an empty answer from a real one
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
        Chosen {
            options: self.ticked.iter().copied().collect(),
            is_own: self.is_own_ticked && has_text,
        }
    }

    fn press_on_list(&mut self, key: &KeyEvent, options: &[Choice], form: Form) -> Effect {
        let choices = options.len();
        if let KeyCode::Char(name) = key.code
            && let Some(index) = options.iter().position(|choice| choice.key == Some(name))
        {
            self.cursor = Cursor::Option(index);
            return Effect::Stay;
        }
        if let Some(to) = move_of(key) {
            return Effect::Go(to);
        }
        let ticks = form != Form::One;
        // Options named by letters take no numbers
        let is_numbered = options.iter().all(|choice| choice.key.is_none());
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
            // The review's rows are named by the questions' keys, which go to them
            (KeyCode::Char('1'..='9'), _, Form::Review) => Effect::Stay,
            (KeyCode::Char(digit @ '1'..='9'), _, _) if is_numbered => {
                let index = usize::from(digit as u8 - b'1');
                if index >= choices {
                    return Effect::Stay;
                }
                self.cursor = Cursor::Option(index);
                if !ticks {
                    return Effect::Stay;
                }
                if !self.ticked.remove(&index) {
                    self.ticked.insert(index);
                }
                Effect::Stay
            }
            (KeyCode::Esc, _, _) => Effect::Back,
            _ => Effect::Stay,
        }
    }

    fn press_in_box(&mut self, key: &KeyEvent, form: Form, input: Input) -> Effect {
        match (key.code, form) {
            (KeyCode::Enter, Form::One) => Effect::Submit,
            // On a question of several options, the text is one more item, which Enter ticks
            (KeyCode::Enter, Form::Several) => {
                self.is_own_ticked = !input.is_empty;
                self.leave_box()
            }
            (KeyCode::Esc, _) if input.is_idle => self.leave_box(),
            (KeyCode::Char(']'), _) if input.is_idle => Effect::Go(Move::Next),
            (KeyCode::Char('['), _) if input.is_idle => Effect::Go(Move::Previous),
            (KeyCode::Char('}'), _) if input.is_idle => Effect::Go(Move::NextRequest),
            (KeyCode::Char('{'), _) if input.is_idle => Effect::Go(Move::PreviousRequest),
            _ => Effect::Type,
        }
    }

    /// Acts on `key` on a submitted question, whose answer stays as it is until Enter opens it.
    pub fn press_submitted(key: &KeyEvent) -> Effect {
        match (key.code, move_of(key)) {
            (_, Some(to)) => Effect::Go(to),
            (KeyCode::Enter, _) => Effect::Submit,
            (KeyCode::Esc, _) => Effect::Back,
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

impl Chosen {
    pub fn is_empty(&self) -> bool {
        self.options.is_empty() && !self.is_own
    }
}

/// The lines of `choices`, and of the item of the user's own answer, which says `own_label`. The
/// pointer marks the item the cursor is on, dimmed while the cursor is in the box, and each item's
/// key takes the width of a number's, `1.`, so the items line up. The review's rows go by the
/// names of their questions instead. A submitted question shows its
/// answer, `submitted`, in green, marked as a tick marks an option, and dims the rest.
pub fn lines(
    choices: &[Choice],
    own_label: &str,
    form: Form,
    pick: &Pick,
    submitted: Option<&Chosen>,
) -> (Vec<Line>, Line) {
    // The start of an item's line, and the style of the rest of it
    let start = |key: &str, cursor: Cursor, is_ticked: bool| {
        let (pointer, style, is_ticked) = match submitted {
            Some(chosen) => {
                let is_chosen = match cursor {
                    Cursor::Option(index) => chosen.options.contains(&index),
                    Cursor::Own | Cursor::Writing => chosen.is_own,
                };
                match (is_chosen, form) {
                    (true, Form::One) => (Span::new("✓", DONE_OR_ADDED), DONE_OR_ADDED, false),
                    (true, _) => (" ".into(), DONE_OR_ADDED, true),
                    (false, _) => (" ".into(), SUBDUED, false),
                }
            }
            None => match (pick.cursor == cursor, pick.cursor) {
                (true, _) => (Span::new("❯", MARK), STRONG, is_ticked),
                (false, Cursor::Writing) if cursor == Cursor::Own => {
                    (Span::new("❯", SUBDUED), PLAIN, is_ticked)
                }
                (false, _) => (" ".into(), PLAIN, is_ticked),
            },
        };
        let tick = match (form, is_ticked) {
            (Form::One, _) => "",
            (_, true) => "[✓] ",
            (_, false) => "[ ] ",
        };
        let start = Line::from([
            pointer,
            " ".into(),
            Span::new(format!("{key:<2} {tick}"), style),
        ]);
        (start, style)
    };
    let options = choices
        .iter()
        .enumerate()
        .map(|(index, choice)| {
            let key = match (form, choice.key) {
                (Form::Review, _) => question_name(index).to_string(),
                (_, Some(key)) => key.to_string(),
                (_, None) => (index + 1).to_string(),
            };
            let (start, style) = start(
                &format!("{key}."),
                Cursor::Option(index),
                pick.ticked.contains(&index),
            );
            let style = if choice.risk.is_some() && submitted.is_none() {
                ATTENTION.on(style)
            } else {
                style
            };
            let line = start.with(Span::new(choice.label.as_str(), style));
            if choice.description.is_empty() {
                line
            } else {
                line.with("  ")
                    .with(Span::new(choice.description.as_str(), SUBDUED))
            }
        })
        .collect();
    if form == Form::Review {
        return (options, Line::default());
    }
    let (own, style) = start(&OWN_KEY.to_string(), Cursor::Own, pick.is_own_ticked);
    (options, own.with(Span::new(own_label, style)))
}

/// The preview of the option the cursor is on.
pub struct Preview {
    /// The key of the option as its line shows it, like `1`, none off the options.
    pub key: Option<String>,
    /// The preview's Markdown, empty when the option has no preview.
    pub markdown: String,
}

/// Whether any option of a question has a preview, which gives every option of it the preview's
/// rows, so the layout stays as the cursor moves.
pub fn has_previews(choices: &[Choice]) -> bool {
    choices.iter().any(|choice| choice.preview.is_some())
}

/// The preview of the option the cursor is on, or none when no option of the question has a
/// preview.
pub fn preview(choices: &[Choice], pick: &Pick) -> Option<Preview> {
    if !has_previews(choices) {
        return None;
    }
    let focused = match pick.cursor {
        Cursor::Option(index) => Some((index, &choices[index])),
        Cursor::Own | Cursor::Writing => None,
    };
    Some(Preview {
        key: focused.map(|(index, choice)| {
            choice
                .key
                .map_or_else(|| (index + 1).to_string(), String::from)
        }),
        markdown: focused
            .and_then(|(_, choice)| choice.preview.clone())
            .unwrap_or_default(),
    })
}

/// The lines of a preview, highlighted as Markdown, since Claude Code shows a preview as
/// Markdown, which Claude then writes it in, without its code fences, which would take 2 of its
/// few rows, while the code's colors already set the code apart. Ctrl+O shows it whole, fences
/// included.
pub fn preview_lines(markdown: &str) -> Vec<Line> {
    let markdown = markdown.trim();
    // The highlighter keeps every line, so each colored line is the one of the Markdown
    (markdown.lines())
        .zip(markdown_lines(markdown))
        .filter(|(line, _)| shows_in_preview(line))
        .map(|(_, colored)| colored)
        .collect()
}

/// The count of `preview_lines(markdown)`, without highlighting them.
pub fn preview_rows(markdown: &str) -> usize {
    (markdown.trim().lines())
        .filter(|line| shows_in_preview(line))
        .count()
}

/// Whether a preview shows `line` of its Markdown: every line but a code fence.
fn shows_in_preview(line: &str) -> bool {
    !is_fence(line)
}

/// The hint of Esc, which goes back from the user's own answer to the options.
pub const GO_BACK: Hint = Hint::new("Esc", "goes back");

/// What the label row says the keys do, after the question's header, with `pick` on `choices`
/// options, where Enter sends the answer at once when `sends_at_once`, and else submits it. On
/// the review, Enter sends the answers when all are ticked, and else opens the unticked ones,
/// which the label says, so Enter never surprises.
pub fn hint(form: Form, pick: &Pick, choices: usize, sends_at_once: bool) -> String {
    let unticked = choices - pick.ticked.len();
    let edit_unticked = format!("edits {unticked}");
    let enter = |does| Hint::new("Enter", does);
    let space = |does| Hint::new("Space", does);
    let answer = enter(if sends_at_once { "sends" } else { "submits" });
    match (form, pick.cursor == Cursor::Writing) {
        (Form::Review, _) if unticked == 0 => hints(&[space("unticks"), enter("sends")]),
        (Form::Review, _) => hints(&[enter(&edit_unticked)]),
        (Form::One, false) => hints(&[answer]),
        (Form::Several, false) => hints(&[space("ticks"), answer]),
        (Form::One, true) => hints(&[answer, GO_BACK]),
        (Form::Several, true) => hints(&[enter("adds"), GO_BACK]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crossterm::event::KeyModifiers;

    const IDLE: Input = Input {
        is_idle: true,
        is_empty: false,
        is_commanding: false,
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
                pick.press(&key, &numbered(3), form, IDLE)
            })
            .collect()
    }

    fn press_code(pick: &mut Pick, form: Form, code: KeyCode, input: Input) -> Effect {
        pick.press(
            &KeyEvent::new(code, KeyModifiers::NONE),
            &numbered(3),
            form,
            input,
        )
    }

    fn numbered(count: usize) -> Vec<Choice> {
        (0..count).map(|_| Choice::new("", "", None)).collect()
    }

    fn decision() -> Vec<Choice> {
        let keyed = |label: &str, key: char| Choice {
            key: Some(key),
            ..Choice::new(label, "", None)
        };
        vec![keyed("allow", 'y'), keyed("refuse", 'n')]
    }

    #[test]
    fn a_letter_only_moves_to_its_option_and_enter_answers() {
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        let mut pick = pick(Cursor::Option(1));
        assert_eq!(
            pick.press(&key(KeyCode::Char('y')), &decision(), Form::One, IDLE),
            Effect::Stay
        );
        assert_eq!(pick.cursor, Cursor::Option(0));
        // Letters name the options, so numbers move to none
        assert_eq!(
            pick.press(&key(KeyCode::Char('2')), &decision(), Form::One, IDLE),
            Effect::Stay
        );
        assert_eq!(pick.cursor, Cursor::Option(0));
        assert_eq!(
            pick.press(&key(KeyCode::Enter), &decision(), Form::One, IDLE),
            Effect::Submit
        );
        let (lines, _) = lines(&decision(), "", Form::One, &pick, None);
        assert!(lines[1].text().contains("n. "));
    }

    #[test]
    fn braces_and_qwer_move_between_requests() {
        let mut pick = pick(Cursor::Option(0));
        assert_eq!(
            press(&mut pick, Form::One, "}{w"),
            [
                Effect::Go(Move::NextRequest),
                Effect::Go(Move::PreviousRequest),
                Effect::Go(Move::ToRequest(1))
            ]
        );
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
    fn a_number_only_moves_to_its_option_or_ticks_it() {
        let mut pick = pick(Cursor::Option(0));
        assert_eq!(press(&mut pick, Form::One, "2"), [Effect::Stay]);
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
        let typing = Input {
            is_idle: false,
            is_empty: false,
            is_commanding: false,
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
        assert_eq!(press(&mut pick, Form::Review, " 1"), [Effect::Stay; 2]);
        assert_eq!(pick.ticked, BTreeSet::from([0, 1]));
        assert_eq!(hint(Form::Review, &pick, 3, false), "Enter edits 1");
    }

    #[test]
    fn a_submitted_question_takes_only_enter_esc_and_brackets() {
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        assert_eq!(Pick::press_submitted(&key(KeyCode::Enter)), Effect::Submit);
        assert_eq!(Pick::press_submitted(&key(KeyCode::Esc)), Effect::Back);
        assert_eq!(
            Pick::press_submitted(&key(KeyCode::Char(']'))),
            Effect::Go(Move::Next)
        );
        for code in [KeyCode::Char('j'), KeyCode::Char('1'), KeyCode::Char(' ')] {
            assert_eq!(Pick::press_submitted(&key(code)), Effect::Stay);
        }
    }

    #[test]
    fn esc_goes_back_and_brackets_and_asdf_move_between_questions() {
        let mut pick = pick(Cursor::Option(0));
        assert_eq!(
            press_code(&mut pick, Form::One, KeyCode::Esc, IDLE),
            Effect::Back
        );
        assert_eq!(
            press(&mut pick, Form::One, "][f"),
            [
                Effect::Go(Move::Next),
                Effect::Go(Move::Previous),
                Effect::Go(Move::To(3))
            ]
        );
        // In the box, they are vi keys
        let mut writing = pick.clone();
        writing.cursor = Cursor::Writing;
        assert_eq!(press(&mut writing, Form::One, "a"), [Effect::Type]);
    }

    #[test]
    fn a_question_with_any_preview_shows_the_preview_of_each_option() {
        let choice = |preview: Option<&str>| Choice::new("", "", preview.map(str::to_string));
        let choices = [choice(Some("a\nb\nc")), choice(None)];
        let first = preview(&choices, &pick(Cursor::Option(0))).expect("a preview");
        assert_eq!(first.key.as_deref(), Some("1"));
        assert_eq!(first.markdown, "a\nb\nc");
        let second = preview(&choices, &pick(Cursor::Option(1))).expect("a preview");
        assert_eq!(second.markdown, "");
        let own = preview(&choices, &pick(Cursor::Own)).expect("a preview");
        assert!(own.key.is_none());
        assert!(preview(&choices[1..], &pick(Cursor::Option(0))).is_none());
    }

    #[test]
    fn a_preview_hides_its_code_fences_and_keeps_its_other_lines() {
        let lines = preview_lines("Intro\n```js\nconst a = 1;\n```\nOutro");
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].text(), "Intro");
        assert_eq!(lines[1].text(), "const a = 1;");
        assert_eq!(lines[2].text(), "Outro");
        assert_eq!(preview_rows("Intro\n```js\nconst a = 1;\n```\nOutro"), 3);
    }

    #[test]
    fn more_options_than_claude_code_allows_take_more_rows() {
        assert_eq!(option_rows(2), OPTION_ROWS);
        assert_eq!(option_rows(6), 6);
    }
}
