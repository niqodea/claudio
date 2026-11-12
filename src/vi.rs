//! The input box's vi: the text, the cursor, the mode, and the keys of the command being typed,
//! which are all a command's pending state. Each key parses those keys again from the start, so
//! every step of a command follows from the keys typed. An operator acts on the range a motion
//! or a text object gives, so every operator works with every motion and object, and the
//! shortcuts like `x` and `D` are an operator with a motion.
//! Ref: <https://github.com/nushell/reedline/blob/main/src/edit_mode/vi/parser.rs>, on parsing
//! the keys typed so far at each key
//! Ref: <https://vimhelp.org/motion.txt.html>, on the motions and their special cases

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    Insert,
    Normal,
    Visual,
    VisualLine,
}

pub struct Editor {
    text: Vec<char>,
    /// An index into the text, which in insert mode can be its length.
    cursor: usize,
    mode: Mode,
    /// The end of the visual selection that stays where `v` or `V` left it.
    anchor: usize,
    /// The keys of the command being typed in normal or visual mode.
    keys: Vec<Key>,
    /// The column `j` and `k` keep across lines of other lengths.
    column: Option<usize>,
    register: Register,
    /// The text before each change, the latest last. The text is a short message virtually all
    /// the time, so a whole copy of it per change costs nothing worth saving.
    undo: Vec<Vec<char>>,
    redo: Vec<Vec<char>>,
    /// Whether a change is under way, which an insert session keeps until Escape, so that one
    /// undo takes the whole session back.
    is_changing: bool,
    /// The last `f`, `F`, `t` or `T`, which `;` and `,` repeat.
    last_find: Option<(Find, char)>,
    /// The last change, which `.` repeats.
    last_change: Option<Change>,
    /// The change whose insert session runs, with the keys typed in it so far.
    typing: Option<Change>,
    /// Where the insert session began, or where the cursor keys last moved it to.
    insert_start: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Key {
    Char(char),
    Control(char),
    Escape,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
}

#[derive(Clone, Default)]
struct Register {
    text: Vec<char>,
    is_linewise: bool,
}

/// A change, with the keys its insert session typed.
#[derive(Clone)]
struct Change {
    command: Command,
    typed: Vec<Key>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Command {
    Move(Motion, Option<usize>),
    Operate {
        operator: Operator,
        target: Target,
        count: Option<usize>,
    },
    Replace(char, Option<usize>),
    ToggleCase(Option<usize>),
    Join(Option<usize>),
    Put {
        before: bool,
        count: Option<usize>,
    },
    Insert(Insert),
    /// Enters the mode, or leaves it for normal mode when the editor is in it.
    Visual(Mode),
    VisualOperate(Operator),
    VisualToggleCase,
    VisualJoin,
    VisualPut,
    VisualObject(Object),
    Undo,
    Redo,
    Repeat(Option<usize>),
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Operator {
    Delete,
    Change,
    Yank,
}

/// What an operator acts on.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Target {
    Motion(Motion),
    Object(Object),
    /// The operator typed twice, like `dd`, which takes whole lines.
    Lines,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Motion {
    Left,
    Right,
    Down,
    Up,
    /// `w`, or `W` for a WORD, which only whitespace ends.
    WordStart(bool),
    WordBack(bool),
    WordEnd(bool),
    LineStart,
    FirstNonBlank,
    LineEnd,
    FirstLine,
    LastLine,
    Find(Find, char),
    /// `;`, or `,` to repeat it the other way.
    RepeatFind(bool),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Find {
    /// `f`.
    Forward,
    /// `t`.
    ForwardBefore,
    /// `F`.
    Backward,
    /// `T`.
    BackwardAfter,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Object {
    is_around: bool,
    kind: ObjectKind,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum ObjectKind {
    /// A word, or a WORD when true.
    Word(bool),
    Quote(char),
    Pair(char, char),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Insert {
    Before,
    After,
    LineStart,
    LineEnd,
    LineBelow,
    LineAbove,
}

/// How an operator takes the text between the cursor and where a motion goes.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Exclusive,
    Inclusive,
    Linewise,
}

/// The text an operator acts on, from `start` to before `end`. A linewise range spans whole
/// lines, from the start of the first to the end of the last, without its line break.
#[derive(Clone, Copy)]
struct Range {
    start: usize,
    end: usize,
    is_linewise: bool,
}

#[derive(Debug, PartialEq)]
enum Parse {
    Incomplete,
    Invalid,
    Command(Command),
}

impl Editor {
    /// An editor in insert mode, with the cursor at the end of `text`.
    pub fn new(text: &str) -> Self {
        let text: Vec<char> = text.chars().collect();
        Editor {
            cursor: text.len(),
            text,
            mode: Mode::Insert,
            anchor: 0,
            keys: Vec::new(),
            column: None,
            register: Register::default(),
            undo: Vec::new(),
            redo: Vec::new(),
            is_changing: false,
            last_find: None,
            last_change: None,
            typing: None,
            insert_start: None,
        }
    }

    pub fn text(&self) -> String {
        self.text.iter().collect()
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// The cursor's index among the text's characters.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// The visual selection's characters, from the first to before the last, none outside
    /// visual mode.
    pub fn selection(&self) -> Option<(usize, usize)> {
        let range = self.visual_range()?;
        Some((range.start, range.end))
    }

    /// The keys of the command being typed, as vim's `showcmd` shows them, empty when none is.
    /// A command is half typed only after plain characters, like a count, an operator, `g` or
    /// `f`, since any other key completes or cancels it, so its keys are all plain characters.
    pub fn pending(&self) -> String {
        self.keys
            .iter()
            .filter_map(|key| match key {
                Key::Char(character) => Some(*character),
                _ => None,
            })
            .collect()
    }

    /// Drops the command being typed, which leaves the text and the mode as they are.
    pub fn cancel(&mut self) {
        self.keys.clear();
    }

    /// Replaces the text with `text`, as one change, in insert mode with the cursor at its end.
    pub fn replace(&mut self, text: &str) {
        self.keys.clear();
        self.begin_change();
        self.text = text.chars().collect();
        self.cursor = self.text.len();
        self.mode = Mode::Insert;
    }

    /// Inserts pasted text at the cursor, with its line breaks as `\n`.
    pub fn paste(&mut self, text: &str) {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let characters: Vec<char> = text.chars().collect();
        if characters.is_empty() {
            return;
        }
        self.keys.clear();
        self.begin_change();
        let at = self.cursor;
        self.text.splice(at..at, characters.iter().copied());
        self.cursor = at + characters.len();
        if self.mode != Mode::Insert {
            self.cursor -= 1;
            self.is_changing = false;
        }
    }

    pub fn press(&mut self, event: &KeyEvent) {
        let Some(key) = key_of(event) else {
            return;
        };
        if self.mode == Mode::Insert {
            self.type_key(key);
            return;
        }
        self.keys.push(key);
        match parse(&self.keys, self.is_visual()) {
            Parse::Incomplete => {}
            Parse::Invalid => self.keys.clear(),
            Parse::Command(command) => {
                self.keys.clear();
                self.execute(command);
            }
        }
    }

    fn is_visual(&self) -> bool {
        matches!(self.mode, Mode::Visual | Mode::VisualLine)
    }

    fn execute(&mut self, command: Command) {
        match command {
            Command::Move(motion, count) => {
                if let Some((to, _)) = self.target(motion, count) {
                    self.cursor = to;
                }
            }
            Command::Operate {
                operator,
                target,
                count,
            } => {
                let range = match target {
                    Target::Motion(motion) => self.motion_range(operator, motion, count),
                    Target::Object(object) => self.object_range(object),
                    Target::Lines => Some(self.lines_range(count)),
                };
                if let Some(range) = range {
                    self.operate(operator, range);
                }
            }
            Command::Replace(character, count) => self.replace_characters(character, count),
            Command::ToggleCase(count) => {
                let end = (self.cursor + count.unwrap_or(1)).min(self.line_end(self.cursor));
                self.toggle_case(self.cursor, end);
                self.cursor = end;
            }
            Command::Join(count) => self.join(count.unwrap_or(2).max(2) - 1),
            Command::Put { before, count } => self.put(before, count.unwrap_or(1)),
            Command::Insert(insert) => self.insert(insert),
            Command::Visual(mode) => {
                if self.mode == mode {
                    self.mode = Mode::Normal;
                } else {
                    if !self.is_visual() {
                        self.anchor = self.cursor;
                    }
                    self.mode = mode;
                }
            }
            Command::VisualOperate(operator) => {
                if let Some(range) = self.visual_range() {
                    self.mode = Mode::Normal;
                    self.operate(operator, range);
                }
            }
            Command::VisualToggleCase => {
                if let Some(range) = self.visual_range() {
                    self.toggle_case(range.start, range.end);
                    self.cursor = range.start;
                    self.mode = Mode::Normal;
                }
            }
            Command::VisualJoin => {
                if let Some(range) = self.visual_range() {
                    let lines = self.line_of(range.end) - self.line_of(range.start);
                    self.cursor = range.start;
                    self.mode = Mode::Normal;
                    self.join(lines.max(1));
                }
            }
            Command::VisualPut => self.put_over_selection(),
            Command::VisualObject(object) => {
                if let Some(range) = self.object_range(object) {
                    self.anchor = range.start;
                    self.cursor = range.end.saturating_sub(1).max(range.start);
                    self.mode = Mode::Visual;
                }
            }
            Command::Undo => self.undo(),
            Command::Redo => self.redo(),
            Command::Repeat(count) => self.repeat(count),
            Command::Cancel => {}
        }
        if !matches!(command, Command::Move(Motion::Down | Motion::Up, _)) {
            self.column = None;
        }
        if is_change(command) {
            let change = Change {
                command,
                typed: Vec::new(),
            };
            if self.mode == Mode::Insert {
                self.typing = Some(change);
            } else {
                self.last_change = Some(change);
            }
        }
        if self.mode != Mode::Insert {
            self.is_changing = false;
            self.clamp();
        }
    }

    /// Acts on a key in insert mode.
    fn type_key(&mut self, key: Key) {
        if key == Key::Escape {
            self.is_changing = false;
            self.mode = Mode::Normal;
            if self.cursor > self.line_start(self.cursor) {
                self.cursor -= 1;
            }
            if let Some(change) = self.typing.take() {
                self.last_change = Some(change);
            }
            return;
        }
        if let Some(change) = &mut self.typing {
            change.typed.push(key);
        }
        match key {
            Key::Char(character) => self.insert_text(&[character]),
            Key::Backspace | Key::Control('h') => {
                if self.cursor > 0 {
                    self.delete(self.cursor - 1, self.cursor);
                }
            }
            Key::Delete => {
                if self.cursor < self.text.len() {
                    self.delete(self.cursor, self.cursor + 1);
                }
            }
            Key::Control('w') => {
                let line_start = self.line_start(self.cursor);
                let start = if self.cursor == line_start {
                    self.cursor.saturating_sub(1)
                } else {
                    self.word_back(self.cursor, false).max(line_start)
                };
                self.delete(self.stop_at_insert_start(start), self.cursor);
            }
            Key::Control('u') => {
                let start = self.stop_at_insert_start(self.line_start(self.cursor));
                self.delete(start, self.cursor);
            }
            Key::Left => {
                if self.cursor > self.line_start(self.cursor) {
                    self.cursor -= 1;
                }
            }
            Key::Right => {
                if self.cursor < self.line_end(self.cursor) {
                    self.cursor += 1;
                }
            }
            Key::Up | Key::Down => {
                let motion = if key == Key::Up {
                    Motion::Up
                } else {
                    Motion::Down
                };
                if let Some((to, _)) = self.target(motion, None) {
                    self.cursor = to;
                }
            }
            Key::Home => self.cursor = self.line_start(self.cursor),
            Key::End => self.cursor = self.line_end(self.cursor),
            Key::Control(_) | Key::Escape => {}
        }
        // The cursor keys start the insert again where they leave the cursor, as in vim
        if matches!(
            key,
            Key::Left | Key::Right | Key::Up | Key::Down | Key::Home | Key::End
        ) {
            self.insert_start = Some(self.cursor);
        }
    }

    /// Where Ctrl-W or Ctrl-U deleting back to `start` stops: once at the insert session's
    /// start, which the next press goes past, as in vim.
    fn stop_at_insert_start(&self, start: usize) -> usize {
        match self.insert_start {
            Some(insert_start) if self.cursor > insert_start => start.max(insert_start),
            _ => start,
        }
    }

    fn insert_text(&mut self, characters: &[char]) {
        self.begin_change();
        let at = self.cursor;
        self.text.splice(at..at, characters.iter().copied());
        self.cursor = at + characters.len();
    }

    /// Deletes the characters from `start` to before `end`, and puts the cursor at `start`.
    fn delete(&mut self, start: usize, end: usize) {
        if start >= end {
            return;
        }
        self.begin_change();
        self.text.drain(start..end);
        self.cursor = start;
    }

    fn begin_change(&mut self) {
        if !self.is_changing {
            self.undo.push(self.text.clone());
            self.redo.clear();
            self.is_changing = true;
        }
    }

    fn undo(&mut self) {
        if let Some(text) = self.undo.pop() {
            let current = std::mem::replace(&mut self.text, text);
            self.cursor = first_difference(&current, &self.text);
            self.redo.push(current);
        }
    }

    fn redo(&mut self) {
        if let Some(text) = self.redo.pop() {
            let current = std::mem::replace(&mut self.text, text);
            self.cursor = first_difference(&current, &self.text);
            self.undo.push(current);
        }
    }

    /// Repeats the last change, with `count` in place of its own when given.
    fn repeat(&mut self, count: Option<usize>) {
        let Some(change) = self.last_change.clone() else {
            return;
        };
        let command = match (change.command, count) {
            (command, None) => command,
            (
                Command::Operate {
                    operator, target, ..
                },
                count,
            ) => Command::Operate {
                operator,
                target,
                count,
            },
            (Command::Replace(character, _), count) => Command::Replace(character, count),
            (Command::ToggleCase(_), count) => Command::ToggleCase(count),
            (Command::Join(_), count) => Command::Join(count),
            (Command::Put { before, .. }, count) => Command::Put { before, count },
            (command, _) => command,
        };
        self.execute(command);
        if self.mode == Mode::Insert {
            for key in change.typed {
                self.type_key(key);
            }
            self.type_key(Key::Escape);
        }
    }

    /// Keeps the cursor on a character outside insert mode: on the last one of its line, at
    /// most, or at the start of an empty line.
    fn clamp(&mut self) {
        self.cursor = self.cursor.min(self.text.len());
        let start = self.line_start(self.cursor);
        let end = self.line_end(self.cursor);
        if self.cursor >= end && end > start {
            self.cursor = end - 1;
        }
    }

    fn operate(&mut self, operator: Operator, range: Range) {
        self.register = Register {
            text: self.text[range.start..range.end].to_vec(),
            is_linewise: range.is_linewise,
        };
        match operator {
            Operator::Yank => {
                if range.is_linewise {
                    self.cursor = self.cursor.max(range.start).min(range.end);
                    if self.line_of(range.start) < self.line_of(self.cursor) {
                        self.cursor = self.first_non_blank(range.start);
                    }
                } else {
                    self.cursor = range.start;
                }
            }
            Operator::Delete if range.is_linewise => {
                // The lines go with one line break: the one after them, or before them at the
                // end of the text
                let (start, end) = if range.end < self.text.len() {
                    (range.start, range.end + 1)
                } else {
                    (range.start.saturating_sub(1), range.end)
                };
                self.delete(start, end);
                let start = self.line_start(start.min(self.text.len()));
                self.cursor = self.first_non_blank(start);
            }
            Operator::Delete => self.delete(range.start, range.end),
            Operator::Change => {
                // Changing whole lines keeps the first one's indentation, as Vim does with
                // autoindent
                let start = if range.is_linewise {
                    self.first_non_blank(range.start)
                } else {
                    range.start
                };
                self.begin_change();
                self.text.drain(start..range.end);
                self.cursor = start;
                self.insert_start = Some(start);
                self.mode = Mode::Insert;
            }
        }
    }

    /// The range from the cursor to where `motion` goes, as `operator` takes it.
    fn motion_range(
        &mut self,
        operator: Operator,
        motion: Motion,
        count: Option<usize>,
    ) -> Option<Range> {
        let from = self.cursor;
        let (mut to, mut kind) = self.target(motion, count)?;
        if let Motion::WordStart(is_big) = motion {
            let is_on_word = self.text.get(from).is_some_and(|c| !c.is_whitespace());
            if operator == Operator::Change && is_on_word {
                // cw and cW change to the end of the word, as ce and cE
                to = from;
                for step in 0..count.unwrap_or(1) {
                    let ends_here = self
                        .text
                        .get(to + 1)
                        .is_none_or(|&next| class(next, is_big) != class(self.text[to], is_big));
                    if !(step == 0 && ends_here) {
                        to = self.word_end(to, is_big);
                    }
                }
                kind = Kind::Inclusive;
            } else {
                // The last word moved over ends the text at its line's end, not at the next
                // line's first word
                let mut word_end = to;
                while word_end > from && self.text[word_end - 1].is_whitespace() {
                    word_end -= 1;
                }
                if word_end > from && self.line_of(to) > self.line_of(word_end - 1) {
                    to = self.line_end(word_end - 1);
                }
            }
        }
        let (start, end) = (from.min(to), from.max(to));
        Some(match kind {
            Kind::Linewise => self.lines_between(start, end),
            Kind::Inclusive => Range {
                start,
                end: (end + 1).min(self.line_end(end)),
                is_linewise: false,
            },
            Kind::Exclusive => Range {
                start,
                end,
                is_linewise: false,
            },
        })
    }

    /// The lines from the cursor's on, `count` of them.
    fn lines_range(&self, count: Option<usize>) -> Range {
        let last = (self.line_of(self.cursor) + count.unwrap_or(1) - 1).min(self.line_count() - 1);
        self.lines_between(self.cursor, self.line_bounds(last).0)
    }

    fn lines_between(&self, first: usize, last: usize) -> Range {
        Range {
            start: self.line_start(first),
            end: self.line_end(last),
            is_linewise: true,
        }
    }

    fn visual_range(&self) -> Option<Range> {
        let (start, end) = (self.anchor.min(self.cursor), self.anchor.max(self.cursor));
        match self.mode {
            Mode::Visual => Some(Range {
                start,
                end: (end + 1).min(self.text.len()),
                is_linewise: false,
            }),
            Mode::VisualLine => Some(self.lines_between(start, end)),
            Mode::Insert | Mode::Normal => None,
        }
    }

    /// Where `motion` takes the cursor, `count` times, with how an operator takes the text
    /// between. None when the motion goes nowhere, like `f` to a character the line lacks.
    fn target(&mut self, motion: Motion, count: Option<usize>) -> Option<(usize, Kind)> {
        let times = count.unwrap_or(1);
        let at = self.cursor;
        let start = self.line_start(at);
        let end = self.line_end(at);
        let to = match motion {
            Motion::Left => {
                let to = at.saturating_sub(times).max(start);
                return (to < at).then_some((to, Kind::Exclusive));
            }
            Motion::Right => {
                let to = (at + times).min(end);
                return (to > at).then_some((to, Kind::Exclusive));
            }
            Motion::Down | Motion::Up => {
                let line = self.line_of(at);
                let target = if motion == Motion::Down {
                    (line + times).min(self.line_count() - 1)
                } else {
                    line.saturating_sub(times)
                };
                if target == line {
                    return None;
                }
                let column = *self.column.get_or_insert(at - start);
                let (target_start, target_end) = self.line_bounds(target);
                let last = target_end.saturating_sub(1).max(target_start);
                return Some(((target_start + column).min(last), Kind::Linewise));
            }
            Motion::WordStart(is_big) => {
                let to = (0..times).fold(at, |to, _| self.word_start(to, is_big));
                return (to > at).then_some((to, Kind::Exclusive));
            }
            Motion::WordBack(is_big) => {
                let to = (0..times).fold(at, |to, _| self.word_back(to, is_big));
                return (to < at).then_some((to, Kind::Exclusive));
            }
            Motion::WordEnd(is_big) => {
                let to = (0..times).fold(at, |to, _| self.word_end(to, is_big));
                return (to > at).then_some((to, Kind::Inclusive));
            }
            Motion::LineStart => return Some((start, Kind::Exclusive)),
            Motion::FirstNonBlank => return Some((self.first_non_blank(start), Kind::Exclusive)),
            Motion::LineEnd => {
                let line = (self.line_of(at) + times - 1).min(self.line_count() - 1);
                let (line_start, line_end) = self.line_bounds(line);
                return Some((line_end.saturating_sub(1).max(line_start), Kind::Inclusive));
            }
            Motion::FirstLine | Motion::LastLine => {
                let last = self.line_count() - 1;
                let line = match (motion, count) {
                    (_, Some(number)) => (number - 1).min(last),
                    (Motion::FirstLine, None) => 0,
                    (_, None) => last,
                };
                let line_start = self.line_bounds(line).0;
                return Some((self.first_non_blank(line_start), Kind::Linewise));
            }
            Motion::Find(find, character) => {
                self.last_find = Some((find, character));
                self.find(find, character, times)?
            }
            Motion::RepeatFind(is_reversed) => {
                let (find, character) = self.last_find?;
                let find = if is_reversed { reverse(find) } else { find };
                self.find(find, character, times)?
            }
        };
        Some(to)
    }

    /// The `times`-th `character` from the cursor on its line, as `find` goes.
    fn find(&self, find: Find, character: char, times: usize) -> Option<(usize, Kind)> {
        let at = self.cursor;
        let (start, end) = (self.line_start(at), self.line_end(at));
        match find {
            Find::Forward | Find::ForwardBefore => {
                let found = (at + 1..end)
                    .filter(|&index| self.text[index] == character)
                    .nth(times - 1)?;
                let to = if find == Find::Forward {
                    found
                } else {
                    found - 1
                };
                Some((to, Kind::Inclusive))
            }
            Find::Backward | Find::BackwardAfter => {
                let found = (start..at)
                    .rev()
                    .filter(|&index| self.text[index] == character)
                    .nth(times - 1)?;
                let to = if find == Find::Backward {
                    found
                } else {
                    found + 1
                };
                Some((to, Kind::Exclusive))
            }
        }
    }

    /// The range of a text object around the cursor, none when the cursor is in none.
    fn object_range(&self, object: Object) -> Option<Range> {
        let (start, end) = match object.kind {
            ObjectKind::Word(is_big) => self.word_object(is_big, object.is_around)?,
            ObjectKind::Quote(quote) => {
                let (open, close) = self.quotes(quote)?;
                if object.is_around {
                    // Around quotes takes the whitespace after them, or else before them
                    let blank = |index: usize| matches!(self.text.get(index), Some(' ' | '\t'));
                    let (mut start, mut end) = (open, close + 1);
                    while blank(end) {
                        end += 1;
                    }
                    if end == close + 1 {
                        while start > self.line_start(open) && blank(start - 1) {
                            start -= 1;
                        }
                    }
                    (start, end)
                } else {
                    (open + 1, close)
                }
            }
            ObjectKind::Pair(open, close) => {
                let (open_at, close_at) = self.pair(open, close)?;
                if object.is_around {
                    (open_at, close_at + 1)
                } else {
                    (open_at + 1, close_at)
                }
            }
        };
        Some(Range {
            start,
            end,
            is_linewise: false,
        })
    }

    /// A word, or the whitespace the cursor is on, with the whitespace after it when around,
    /// or else before it.
    fn word_object(&self, is_big: bool, is_around: bool) -> Option<(usize, usize)> {
        let at = self.cursor;
        let (line_start, line_end) = (self.line_start(at), self.line_end(at));
        if at >= line_end {
            return None;
        }
        let kind = class(self.text[at], is_big);
        let same = |index: usize| class(self.text[index], is_big) == kind;
        let mut start = at;
        while start > line_start && same(start - 1) {
            start -= 1;
        }
        let mut end = at + 1;
        while end < line_end && same(end) {
            end += 1;
        }
        if !is_around {
            return Some((start, end));
        }
        let is_blank = |index: usize| class(self.text[index], is_big) == Class::Blank;
        if kind == Class::Blank {
            // Around whitespace takes the word after it too
            let after = class_of(&self.text, end, is_big);
            while end < line_end && class(self.text[end], is_big) == after {
                end += 1;
            }
            return Some((start, end));
        }
        let word_end = end;
        while end < line_end && is_blank(end) {
            end += 1;
        }
        if end == word_end {
            while start > line_start && is_blank(start - 1) {
                start -= 1;
            }
        }
        Some((start, end))
    }

    /// The quotes on the cursor's line around it, or else the first pair after it.
    fn quotes(&self, quote: char) -> Option<(usize, usize)> {
        let at = self.cursor;
        let positions: Vec<usize> = (self.line_start(at)..self.line_end(at))
            .filter(|&index| self.text[index] == quote)
            .collect();
        positions
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&[open, close]| (open, close))
            .find(|&(_, close)| close >= at)
    }

    /// The innermost `open` and `close` around the cursor, across lines.
    fn pair(&self, open: char, close: char) -> Option<(usize, usize)> {
        let at = self.cursor.min(self.text.len().checked_sub(1)?);
        let mut depth = 0;
        let mut index = at;
        let open_at = loop {
            let character = self.text[index];
            if character == close && index != at {
                depth += 1;
            } else if character == open {
                if depth == 0 {
                    break index;
                }
                depth -= 1;
            }
            index = index.checked_sub(1)?;
        };
        let mut depth = 0;
        let close_at = (open_at + 1..self.text.len()).find(|&index| {
            let character = self.text[index];
            if character == open {
                depth += 1;
            } else if character == close {
                if depth == 0 {
                    return true;
                }
                depth -= 1;
            }
            false
        })?;
        Some((open_at, close_at))
    }

    fn replace_characters(&mut self, character: char, count: Option<usize>) {
        let times = count.unwrap_or(1);
        let end = self.cursor + times;
        // Short of characters on its line, r replaces none
        if end > self.line_end(self.cursor) {
            return;
        }
        self.begin_change();
        for index in self.cursor..end {
            self.text[index] = character;
        }
        self.cursor = end - 1;
    }

    fn toggle_case(&mut self, start: usize, end: usize) {
        if start >= end {
            return;
        }
        self.begin_change();
        for index in start..end {
            let character = self.text[index];
            let toggled: Vec<char> = if character.is_uppercase() {
                character.to_lowercase().collect()
            } else {
                character.to_uppercase().collect()
            };
            // A character whose other case takes several characters stays as it is
            if let [toggled] = toggled[..] {
                self.text[index] = toggled;
            }
        }
    }

    /// Joins the cursor's line with the `joins` lines below it, with one space between each
    /// two, where the line below has text.
    fn join(&mut self, joins: usize) {
        for _ in 0..joins {
            let line_end = self.line_end(self.cursor);
            if line_end >= self.text.len() {
                return;
            }
            self.begin_change();
            let mut next = line_end + 1;
            while next < self.text.len() && matches!(self.text[next], ' ' | '\t') {
                next += 1;
            }
            let is_next_empty = next >= self.text.len() || self.text[next] == '\n';
            let is_left_empty = line_end == self.line_start(line_end);
            let separator: &[char] = if is_next_empty || is_left_empty {
                &[]
            } else {
                &[' ']
            };
            self.text.splice(line_end..next, separator.iter().copied());
            self.cursor = line_end;
        }
    }

    fn put(&mut self, before: bool, times: usize) {
        if self.register.text.is_empty() && !self.register.is_linewise {
            return;
        }
        self.begin_change();
        let text = &self.register.text;
        if self.register.is_linewise {
            let mut block = Vec::new();
            for _ in 0..times {
                block.extend_from_slice(text);
                block.push('\n');
            }
            let at = if before {
                self.line_start(self.cursor)
            } else {
                // The block goes after the line's break, which the text's last line lacks
                block.pop();
                block.insert(0, '\n');
                self.line_end(self.cursor)
            };
            self.text.splice(at..at, block);
            let first_line = if before { at } else { at + 1 };
            self.cursor = self.first_non_blank(first_line);
        } else {
            let at = if before || self.line_start(self.cursor) == self.line_end(self.cursor) {
                self.cursor
            } else {
                self.cursor + 1
            };
            let block: Vec<char> = text.repeat(times);
            let length = block.len();
            self.text.splice(at..at, block);
            self.cursor = at + length - 1;
        }
    }

    /// Puts the register in place of the visual selection, which then fills the register.
    fn put_over_selection(&mut self) {
        let Some(range) = self.visual_range() else {
            return;
        };
        let register = self.register.clone();
        self.mode = Mode::Normal;
        self.begin_change();
        let removed = self
            .text
            .splice(range.start..range.end, register.text.iter().copied());
        let removed = removed.collect();
        self.register = Register {
            text: removed,
            is_linewise: range.is_linewise,
        };
        self.cursor = (range.start + register.text.len()).saturating_sub(1);
    }

    fn insert(&mut self, insert: Insert) {
        self.begin_change();
        let (start, end) = (self.line_start(self.cursor), self.line_end(self.cursor));
        self.cursor = match insert {
            Insert::Before => self.cursor,
            Insert::After => (self.cursor + 1).min(end),
            Insert::LineStart => self.first_non_blank(start),
            Insert::LineEnd => end,
            Insert::LineBelow => {
                self.text.insert(end, '\n');
                end + 1
            }
            Insert::LineAbove => {
                self.text.insert(start, '\n');
                start
            }
        };
        self.insert_start = Some(self.cursor);
        self.mode = Mode::Insert;
    }

    /// The start of the next word from `at`, or the start of an empty line, or the text's end.
    fn word_start(&self, at: usize, is_big: bool) -> usize {
        let length = self.text.len();
        let mut index = at;
        let kind = class_of(&self.text, at, is_big);
        if kind != Class::Blank {
            while index < length && class(self.text[index], is_big) == kind {
                index += 1;
            }
        }
        while index < length && class(self.text[index], is_big) == Class::Blank {
            if self.text[index] == '\n' && self.text.get(index + 1) == Some(&'\n') {
                return index + 1;
            }
            index += 1;
        }
        index
    }

    /// The start of the word before `at`, or of an empty line between them.
    fn word_back(&self, at: usize, is_big: bool) -> usize {
        let Some(mut index) = at.checked_sub(1) else {
            return 0;
        };
        while index > 0 && class(self.text[index], is_big) == Class::Blank {
            if self.text[index] == '\n' && self.text[index - 1] == '\n' {
                return index;
            }
            index -= 1;
        }
        let kind = class(self.text[index], is_big);
        while index > 0 && class(self.text[index - 1], is_big) == kind {
            index -= 1;
        }
        index
    }

    /// The end of the word after `at`, or of the word `at` is in when it ends further.
    fn word_end(&self, at: usize, is_big: bool) -> usize {
        let length = self.text.len();
        let mut index = at + 1;
        while index < length && class(self.text[index], is_big) == Class::Blank {
            index += 1;
        }
        if index >= length {
            return at;
        }
        let kind = class(self.text[index], is_big);
        while index + 1 < length && class(self.text[index + 1], is_big) == kind {
            index += 1;
        }
        index
    }

    fn line_start(&self, at: usize) -> usize {
        self.text[..at.min(self.text.len())]
            .iter()
            .rposition(|&character| character == '\n')
            .map_or(0, |line_break| line_break + 1)
    }

    /// The index of the line break that ends the line `at` is on, or the text's length.
    fn line_end(&self, at: usize) -> usize {
        let at = at.min(self.text.len());
        self.text[at..]
            .iter()
            .position(|&character| character == '\n')
            .map_or(self.text.len(), |offset| at + offset)
    }

    fn line_of(&self, at: usize) -> usize {
        self.text[..at.min(self.text.len())]
            .iter()
            .filter(|&&character| character == '\n')
            .count()
    }

    fn line_count(&self) -> usize {
        self.line_of(self.text.len()) + 1
    }

    /// The start and the end of the line with `number`, counted from 0.
    fn line_bounds(&self, number: usize) -> (usize, usize) {
        let start = if number == 0 {
            0
        } else {
            self.text
                .iter()
                .enumerate()
                .filter(|&(_, &character)| character == '\n')
                .nth(number - 1)
                .map_or(self.text.len(), |(index, _)| index + 1)
        };
        (start, self.line_end(start))
    }

    fn first_non_blank(&self, line_start: usize) -> usize {
        let end = self.line_end(line_start);
        (line_start..end)
            .find(|&index| !matches!(self.text[index], ' ' | '\t'))
            .unwrap_or(end)
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Class {
    Blank,
    Word,
    Punctuation,
}

/// Whether `character` is whitespace, part of a word, or punctuation. A WORD is anything but
/// whitespace.
fn class(character: char, is_big: bool) -> Class {
    if character.is_whitespace() {
        Class::Blank
    } else if is_big || character.is_alphanumeric() || character == '_' {
        Class::Word
    } else {
        Class::Punctuation
    }
}

/// The class of the character at `at`, whitespace past the text's end.
fn class_of(text: &[char], at: usize, is_big: bool) -> Class {
    text.get(at)
        .map_or(Class::Blank, |&character| class(character, is_big))
}

/// Where two texts start to differ, which is where a change undone or redone starts.
fn first_difference(text: &[char], other: &[char]) -> usize {
    text.iter()
        .zip(other)
        .position(|(character, other)| character != other)
        .unwrap_or(text.len().min(other.len()))
}

fn reverse(find: Find) -> Find {
    match find {
        Find::Forward => Find::Backward,
        Find::ForwardBefore => Find::BackwardAfter,
        Find::Backward => Find::Forward,
        Find::BackwardAfter => Find::ForwardBefore,
    }
}

/// Whether `.` repeats the command, which changes the text outside visual mode.
fn is_change(command: Command) -> bool {
    match command {
        Command::Operate { operator, .. } => operator != Operator::Yank,
        Command::Replace(..)
        | Command::ToggleCase(_)
        | Command::Join(_)
        | Command::Put { .. }
        | Command::Insert(_) => true,
        _ => false,
    }
}

fn key_of(event: &KeyEvent) -> Option<Key> {
    let is_control = event.modifiers.contains(KeyModifiers::CONTROL);
    Some(match event.code {
        KeyCode::Char(character) if is_control => Key::Control(character),
        KeyCode::Char(character) => Key::Char(character),
        KeyCode::Esc => Key::Escape,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Delete => Key::Delete,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        _ => return None,
    })
}

/// The command the keys typed so far make in normal or visual mode, or how far they are from
/// one.
fn parse(keys: &[Key], is_visual: bool) -> Parse {
    let (count, keys) = split_count(keys);
    let Some((&first, rest)) = keys.split_first() else {
        return Parse::Incomplete;
    };
    if is_visual {
        return parse_visual(first, rest, keys, count);
    }
    let operate = |operator, target| Command::Operate {
        operator,
        target,
        count,
    };
    let command = match (first, rest) {
        (Key::Char('d' | 'c' | 'y'), _) => return parse_operator(first, rest, count),
        (Key::Char('x') | Key::Delete, []) => {
            operate(Operator::Delete, Target::Motion(Motion::Right))
        }
        (Key::Char('X'), []) => operate(Operator::Delete, Target::Motion(Motion::Left)),
        (Key::Char('D'), []) => operate(Operator::Delete, Target::Motion(Motion::LineEnd)),
        (Key::Char('C'), []) => operate(Operator::Change, Target::Motion(Motion::LineEnd)),
        (Key::Char('s'), []) => operate(Operator::Change, Target::Motion(Motion::Right)),
        (Key::Char('S'), []) => operate(Operator::Change, Target::Lines),
        (Key::Char('r'), []) => return Parse::Incomplete,
        (Key::Char('r'), [Key::Char(character)]) => Command::Replace(*character, count),
        (Key::Char('~'), []) => Command::ToggleCase(count),
        (Key::Char('J'), []) => Command::Join(count),
        (Key::Char('p'), []) => Command::Put {
            before: false,
            count,
        },
        (Key::Char('P'), []) => Command::Put {
            before: true,
            count,
        },
        (Key::Char('u'), []) => Command::Undo,
        (Key::Control('r'), []) => Command::Redo,
        (Key::Char('.'), []) => Command::Repeat(count),
        (Key::Char('i'), []) => Command::Insert(Insert::Before),
        (Key::Char('a'), []) => Command::Insert(Insert::After),
        (Key::Char('I'), []) => Command::Insert(Insert::LineStart),
        (Key::Char('A'), []) => Command::Insert(Insert::LineEnd),
        (Key::Char('o'), []) => Command::Insert(Insert::LineBelow),
        (Key::Char('O'), []) => Command::Insert(Insert::LineAbove),
        (Key::Char('v'), []) => Command::Visual(Mode::Visual),
        (Key::Char('V'), []) => Command::Visual(Mode::VisualLine),
        (Key::Escape, []) => Command::Cancel,
        _ => {
            return match parse_motion(keys) {
                Ok(motion) => Parse::Command(Command::Move(motion, count)),
                Err(parse) => parse,
            };
        }
    };
    Parse::Command(command)
}

/// An operator, then its target: a count, and a motion, a text object, or the operator again.
fn parse_operator(first: Key, rest: &[Key], count: Option<usize>) -> Parse {
    let operator = match first {
        Key::Char('d') => Operator::Delete,
        Key::Char('c') => Operator::Change,
        _ => Operator::Yank,
    };
    let (second_count, rest) = split_count(rest);
    // Counts multiply, so 2d3w deletes six words
    let count = match (count, second_count) {
        (Some(first), Some(second)) => Some(first * second),
        (first, second) => first.or(second),
    };
    let target = match rest {
        [] => return Parse::Incomplete,
        [key] if *key == first => Target::Lines,
        [Key::Char(kind @ ('i' | 'a')), object @ ..] => match parse_object(*kind == 'a', object) {
            Ok(object) => Target::Object(object),
            Err(parse) => return parse,
        },
        motion => match parse_motion(motion) {
            Ok(motion) => Target::Motion(motion),
            Err(parse) => return parse,
        },
    };
    Parse::Command(Command::Operate {
        operator,
        target,
        count,
    })
}

fn parse_visual(first: Key, rest: &[Key], keys: &[Key], count: Option<usize>) -> Parse {
    let command = match (first, rest) {
        (Key::Char('d' | 'x') | Key::Delete, []) => Command::VisualOperate(Operator::Delete),
        (Key::Char('c' | 's'), []) => Command::VisualOperate(Operator::Change),
        (Key::Char('y'), []) => Command::VisualOperate(Operator::Yank),
        (Key::Char('~'), []) => Command::VisualToggleCase,
        (Key::Char('J'), []) => Command::VisualJoin,
        (Key::Char('p' | 'P'), []) => Command::VisualPut,
        (Key::Char('v'), []) => Command::Visual(Mode::Visual),
        (Key::Char('V'), []) => Command::Visual(Mode::VisualLine),
        (Key::Escape, []) => Command::Visual(Mode::Normal),
        (Key::Char(kind @ ('i' | 'a')), object) => match parse_object(kind == 'a', object) {
            Ok(object) => Command::VisualObject(object),
            Err(parse) => return parse,
        },
        _ => {
            return match parse_motion(keys) {
                Ok(motion) => Parse::Command(Command::Move(motion, count)),
                Err(parse) => parse,
            };
        }
    };
    Parse::Command(command)
}

fn parse_motion(keys: &[Key]) -> Result<Motion, Parse> {
    Ok(match keys {
        [] | [Key::Char('g' | 'f' | 't' | 'F' | 'T')] => return Err(Parse::Incomplete),
        [Key::Char('h') | Key::Left | Key::Backspace] => Motion::Left,
        [Key::Char('l' | ' ') | Key::Right] => Motion::Right,
        [Key::Char('j') | Key::Down] => Motion::Down,
        [Key::Char('k') | Key::Up] => Motion::Up,
        [Key::Char('w')] => Motion::WordStart(false),
        [Key::Char('W')] => Motion::WordStart(true),
        [Key::Char('b')] => Motion::WordBack(false),
        [Key::Char('B')] => Motion::WordBack(true),
        [Key::Char('e')] => Motion::WordEnd(false),
        [Key::Char('E')] => Motion::WordEnd(true),
        [Key::Char('0') | Key::Home] => Motion::LineStart,
        [Key::Char('^')] => Motion::FirstNonBlank,
        [Key::Char('$') | Key::End] => Motion::LineEnd,
        [Key::Char('g'), Key::Char('g')] => Motion::FirstLine,
        [Key::Char('G')] => Motion::LastLine,
        [Key::Char(';')] => Motion::RepeatFind(false),
        [Key::Char(',')] => Motion::RepeatFind(true),
        [Key::Char(find), Key::Char(character)] => {
            let find = match find {
                'f' => Find::Forward,
                't' => Find::ForwardBefore,
                'F' => Find::Backward,
                'T' => Find::BackwardAfter,
                _ => return Err(Parse::Invalid),
            };
            Motion::Find(find, *character)
        }
        _ => return Err(Parse::Invalid),
    })
}

fn parse_object(is_around: bool, keys: &[Key]) -> Result<Object, Parse> {
    let kind = match keys {
        [] => return Err(Parse::Incomplete),
        [Key::Char('w')] => ObjectKind::Word(false),
        [Key::Char('W')] => ObjectKind::Word(true),
        [Key::Char(quote @ ('"' | '\'' | '`'))] => ObjectKind::Quote(*quote),
        [Key::Char('(' | ')' | 'b')] => ObjectKind::Pair('(', ')'),
        [Key::Char('[' | ']')] => ObjectKind::Pair('[', ']'),
        [Key::Char('{' | '}' | 'B')] => ObjectKind::Pair('{', '}'),
        [Key::Char('<' | '>')] => ObjectKind::Pair('<', '>'),
        _ => return Err(Parse::Invalid),
    };
    Ok(Object { is_around, kind })
}

/// The count at the start of `keys`, and the keys after it. A count starts with 1 to 9, since 0
/// alone moves to the line's start.
fn split_count(keys: &[Key]) -> (Option<usize>, &[Key]) {
    let digits = keys
        .iter()
        .enumerate()
        .take_while(|&(index, key)| match key {
            Key::Char('1'..='9') => true,
            Key::Char('0') => index > 0,
            _ => false,
        })
        .count();
    if digits == 0 {
        return (None, keys);
    }
    let count = keys[..digits]
        .iter()
        .filter_map(|key| match key {
            Key::Char(digit) => digit.to_digit(10),
            _ => None,
        })
        .fold(0, |count, digit| count * 10 + digit as usize);
    (Some(count), &keys[digits..])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The keys in `keys`, where `<Esc>`, `<BS>` and `<C-x>` name the keys without a
    /// character of their own.
    pub(super) fn events(keys: &str) -> Vec<KeyEvent> {
        let mut events = Vec::new();
        let mut rest = keys;
        while let Some(character) = rest.chars().next() {
            if character == '<'
                && let Some(end) = rest.find('>')
                && end > 1
            {
                let name = &rest[1..end];
                let event = match name {
                    "Esc" => Some(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
                    "BS" => Some(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)),
                    _ => name.strip_prefix("C-").and_then(|key| {
                        let key = key.chars().next()?;
                        Some(KeyEvent::new(KeyCode::Char(key), KeyModifiers::CONTROL))
                    }),
                };
                if let Some(event) = event {
                    events.push(event);
                    rest = &rest[end + 1..];
                    continue;
                }
            }
            events.push(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
            rest = &rest[character.len_utf8()..];
        }
        events
    }

    /// An editor in normal mode on `text` at its first character, after `keys`.
    pub(super) fn after(text: &str, keys: &str) -> Editor {
        let mut editor = Editor::new(text);
        editor.mode = Mode::Normal;
        editor.cursor = 0;
        for event in events(keys) {
            editor.press(&event);
        }
        editor
    }

    /// The text with `|` before the cursor's character.
    fn shown(editor: &Editor) -> String {
        let mut text = editor.text.clone();
        text.insert(editor.cursor, '|');
        text.into_iter().collect()
    }

    #[test]
    fn an_operator_works_with_every_motion() {
        let cases = [
            ("foo bar baz", "dw", "|bar baz"),
            ("foo bar baz", "de", "| bar baz"),
            ("foo bar baz", "wdb", "|bar baz"),
            ("foo bar baz", "d$", "|"),
            ("foo bar baz", "wd0", "|bar baz"),
            ("foo bar baz", "dfa", "|r baz"),
            ("foo bar baz", "dta", "|ar baz"),
            ("foo bar baz", "$dFa", "foo bar b|z"),
            ("foo bar baz", "2dw", "|baz"),
            ("foo bar baz", "d2w", "|baz"),
            ("a b c d e f g", "2d3w", "|g"),
        ];
        for (text, keys, expected) in cases {
            assert_eq!(shown(&after(text, keys)), expected, "{keys} on {text}");
        }
    }

    #[test]
    fn change_word_changes_to_the_word_end() {
        assert_eq!(shown(&after("foo bar baz", "cwX<Esc>")), "|X bar baz");
        assert_eq!(shown(&after("foo bar", "llcwX<Esc>")), "fo|X bar");
    }

    #[test]
    fn delete_word_on_the_last_word_keeps_the_line_break() {
        assert_eq!(shown(&after("foo bar\nbaz", "wdw")), "foo| \nbaz");
    }

    #[test]
    fn text_objects_work_with_every_operator() {
        let cases = [
            ("foo bar baz", "wdiw", "foo | baz"),
            ("foo bar baz", "wdaw", "foo |baz"),
            ("say \"hi there\" now", "fhdi\"", "say \"|\" now"),
            ("say \"hi there\" now", "fhda\"", "say |now"),
            ("f(a, (b))", "fbdi(", "f(a, (|))"),
            ("f(a, (b))", "fadi(", "f(|)"),
            ("x [1, 2] y", "f1da[", "x | y"),
            ("x {a} y", "faci{", "x {|} y"),
        ];
        for (text, keys, expected) in cases {
            assert_eq!(shown(&after(text, keys)), expected, "{keys} on {text}");
        }
    }

    #[test]
    fn whole_lines_go_and_come_back_whole() {
        assert_eq!(shown(&after("one\ntwo\nthree", "jdd")), "one\n|three");
        assert_eq!(shown(&after("one\ntwo\nthree", "Gdd")), "one\n|two");
        assert_eq!(shown(&after("one\ntwo", "ddp")), "two\n|one");
        assert_eq!(shown(&after("one\ntwo", "yyjP")), "one\n|one\ntwo");
        assert_eq!(
            shown(&after("one\ntwo\nthree", "2yyGp")),
            "one\ntwo\nthree\n|one\ntwo"
        );
    }

    #[test]
    fn the_shortcuts_are_an_operator_with_a_motion() {
        let cases = [
            ("abc", "x", "|bc"),
            ("abc", "$X", "a|c"),
            ("abc def", "wD", "abc| "),
            ("abc def", "wCX<Esc>", "abc |X"),
            ("abc", "sX<Esc>", "|Xbc"),
            ("  abc", "SX<Esc>", "  |X"),
            ("", "x", "|"),
        ];
        for (text, keys, expected) in cases {
            assert_eq!(shown(&after(text, keys)), expected, "{keys} on {text}");
        }
    }

    #[test]
    fn replace_takes_a_character_or_a_line_break() {
        assert_eq!(shown(&after("abc", "rx")), "|xbc");
        assert_eq!(shown(&after("abc", "2rx")), "x|xc");
        assert_eq!(shown(&after("abc", "5rx")), "|abc");
    }

    #[test]
    fn the_editor_shows_the_keys_of_the_command_being_typed() {
        for (keys, pending) in [
            ("", ""),
            ("2d", "2d"),
            ("c2T", "c2T"),
            ("di", "di"),
            ("r", "r"),
        ] {
            assert_eq!(after("abc", keys).pending(), pending, "{keys}");
        }
        for keys in ["dw", "rx", "x", "ifoo"] {
            assert_eq!(after("abc", keys).pending(), "", "{keys}");
        }
    }

    #[test]
    fn j_and_k_keep_the_column_across_shorter_lines() {
        let editor = after("abcdef\nab\nabcdef", "5ljj");
        assert_eq!(shown(&editor), "abcdef\nab\nabcde|f");
    }

    #[test]
    fn one_undo_takes_back_a_whole_insert_session() {
        assert_eq!(shown(&after("abc", "Afoo<Esc>u")), "ab|c");
        assert_eq!(shown(&after("abc", "Afoo<Esc>u<C-r>")), "abc|foo");
        assert_eq!(shown(&after("abc def", "dwu")), "|abc def");
    }

    #[test]
    fn dot_repeats_the_last_change_with_a_new_count() {
        assert_eq!(shown(&after("a b c d e f", "2dw.")), "|e f");
        assert_eq!(shown(&after("a b c d e f", "2dw3.")), "|f");
        assert_eq!(shown(&after("a b c", "ciwX<Esc>w.")), "X |X c");
    }

    #[test]
    fn visual_mode_selects_then_operates() {
        assert_eq!(shown(&after("foo bar baz", "wvey")), "foo |bar baz");
        assert_eq!(shown(&after("foo bar baz", "wved")), "foo | baz");
        assert_eq!(shown(&after("one\ntwo\nthree", "Vjd")), "|three");
        assert_eq!(shown(&after("foo bar", "viwc!<Esc>")), "|! bar");
        assert_eq!(shown(&after("abc", "vl~")), "|ABc");
    }

    #[test]
    fn join_puts_one_space_between_lines_with_text() {
        assert_eq!(shown(&after("a\n   b\n\nc", "J")), "a| b\n\nc");
        assert_eq!(shown(&after("a\n\nc", "J")), "|a\nc");
    }

    /// Texts and keys whose outcome the editor shares with Vim: the commands of its grammar, and
    /// Vim's special cases.
    const VIM_CASES: &[(&str, &str)] = &[
        ("foo bar baz", "w"),
        ("foo bar baz", "2w"),
        ("foo.bar baz", "w"),
        ("foo.bar baz", "W"),
        ("foo bar baz", "$b"),
        ("foo bar baz", "$B"),
        ("foo bar baz", "e"),
        ("foo bar baz", "3e"),
        ("foo bar\nbaz qux", "3w"),
        ("foo\n\nbar", "w"),
        ("foo\n\nbar", "Gb"),
        ("  foo bar", "$^"),
        ("foo bar", "$0"),
        ("abcdef\nab\nabcdef", "5ljj"),
        ("abcdef\nab\nabcdef", "5ljjk"),
        ("one\ntwo\nthree", "G"),
        ("one\ntwo\nthree", "2G"),
        ("one\ntwo\nthree", "Ggg"),
        ("foo bar baz", "fa"),
        ("foo bar baz", "2fa"),
        ("foo bar baz", "ta"),
        ("foo bar baz", "fa;"),
        ("foo bar baz", "$Fa"),
        ("foo bar baz", "$Ta"),
        ("foo bar baz", "fa;,"),
        ("foo bar baz", "dw"),
        ("foo bar baz", "de"),
        ("foo bar baz", "wdb"),
        ("foo bar baz", "d$"),
        ("foo bar baz", "wd0"),
        ("foo bar baz", "dfa"),
        ("foo bar baz", "dta"),
        ("foo bar baz", "$dFa"),
        ("foo bar baz", "2dw"),
        ("foo bar baz", "d2w"),
        ("a b c d e f g", "2d3w"),
        ("foo bar\nbaz", "wdw"),
        ("foo bar\nbaz", "wd2w"),
        ("foo bar baz", "cwX\x1b"),
        ("foo bar baz", "llcwX\x1b"),
        ("foo bar baz", "c2wX\x1b"),
        ("foo  bar", "lllcwX\x1b"),
        ("foo bar baz", "wdiw"),
        ("foo bar baz", "wdaw"),
        ("foo bar baz", "$daw"),
        ("foo bar baz", "wyiwP"),
        ("foo bar baz", "wciWX\x1b"),
        ("say \"hi there\" now", "fhdi\""),
        ("say \"hi there\" now", "fhda\""),
        ("say \"hi\"", "fhda\""),
        ("f(a, (b))", "fbdi("),
        ("f(a, (b))", "fadi("),
        ("f(a, (b))", "fada("),
        ("x [1, 2] y", "f1da["),
        ("x {a} y", "faci{X\x1b"),
        ("<a b>", "fbdi<"),
        ("one\ntwo\nthree", "jdd"),
        ("one\ntwo\nthree", "Gdd"),
        ("one\ntwo\nthree", "2dd"),
        ("one\ntwo\nthree", "dj"),
        ("one\ntwo\nthree", "Gdk"),
        ("one\ntwo", "ddp"),
        ("one\ntwo", "yyjP"),
        ("one\ntwo\nthree", "2yyGp"),
        ("one\ntwo\nthree", "jccX\x1b"),
        ("foo bar", "yiwP"),
        ("foo bar", "yiw$p"),
        ("foo bar", "yw3p"),
        ("abc", "x"),
        ("abc", "3x"),
        ("abc", "$X"),
        ("abc def", "wD"),
        ("abc def", "wCX\x1b"),
        ("abc", "sX\x1b"),
        ("  abc", "SX\x1b"),
        ("", "x"),
        ("abc", "rx"),
        ("abc", "2rx"),
        ("abc", "5rx"),
        ("abc", "~~"),
        ("abc", "3~"),
        ("a\n   b\n\nc", "J"),
        ("a\nb\nc", "3J"),
        ("a\n\nc", "J"),
        ("abc", "ifoo\x1b"),
        ("abc", "afoo\x1b"),
        ("  abc", "Ifoo\x1b"),
        ("abc", "Afoo\x1b"),
        ("abc\ndef", "ofoo\x1b"),
        ("abc\ndef", "jOfoo\x1b"),
        ("abc", "Afoo\x1bu"),
        ("abc def", "dwu"),
        ("abc def", "dwu\x12"),
        ("abc def", "xxuu"),
        ("a b c d e f", "2dw."),
        ("a b c d e f", "2dw3."),
        ("a b c", "ciwX\x1bw."),
        ("one\ntwo\nthree", "dd."),
        ("abc", "Afoo\x1bj."),
        ("foo bar baz", "wvey"),
        ("foo bar baz", "wved"),
        ("one\ntwo\nthree", "Vjd"),
        ("one\ntwo\nthree", "jVy"),
        ("foo bar", "viwc!\x1b"),
        ("abc", "vl~"),
        ("one\ntwo\nthree", "VjJ"),
        ("foo bar", "yiwwviwp"),
        ("foo bar baz", "wvawd"),
        ("abc", "ixy\x08z\x1b"),
        ("abc def", "Ax\x17y\x1b"),
        ("abc def", "Ax\x15y\x1b"),
    ];

    /// Runs `keys` on `text` in Vim, from normal mode at its first character, and returns what
    /// the comparison takes: the text, the cursor's line and column, from 1, and the register's
    /// type, `v` for characters, `V` for lines. Vim runs without a screen, in its silent Ex
    /// mode, with autoindent, which the editor has.
    fn vim(text: &str, keys: &str) -> String {
        let lines: Vec<String> = text
            .split('\n')
            .map(|line| format!("'{}'", line.replace('\'', "''")))
            .collect();
        let keys: String = keys
            .chars()
            .map(|key| match key {
                '\x1b' => "\\<Esc>".to_string(),
                '\x08' => "\\<BS>".to_string(),
                '\x12' => "\\<C-r>".to_string(),
                '\x17' => "\\<C-w>".to_string(),
                '\x15' => "\\<C-u>".to_string(),
                '"' => "\\\"".to_string(),
                '\\' => "\\\\".to_string(),
                character => character.to_string(),
            })
            .collect();
        let output = std::process::Command::new("vim")
            .args([
                "--clean",
                "-es",
                "-c",
                "set autoindent",
                "-c",
                &format!("call setline(1, [{}])", lines.join(", ")),
                "-c",
                &format!("call feedkeys(\"{keys}\", 'tx')"),
                "-c",
                "call writefile(getline(1, '$') + [line('.') . ' ' . col('.'), getregtype('\"')], '/dev/stdout')",
                "-c",
                "qa!",
            ])
            .output()
            .expect("vim runs");
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// The same as `vim`, from the editor.
    fn ours(text: &str, keys: &str) -> String {
        let mut editor = Editor::new(text);
        editor.mode = Mode::Normal;
        editor.cursor = 0;
        for key in keys.chars() {
            let event = match key {
                '\x1b' => KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                '\x08' => KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
                '\x12' => KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL),
                '\x17' => KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL),
                '\x15' => KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
                character => KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
            };
            editor.press(&event);
        }
        let line = editor.line_of(editor.cursor) + 1;
        let column = editor.cursor - editor.line_start(editor.cursor) + 1;
        let register = match (editor.register.is_linewise, editor.register.text.is_empty()) {
            (true, _) => "V",
            (false, false) => "v",
            (false, true) => "",
        };
        format!("{}\n{line} {column}\n{register}\n", editor.text())
    }

    #[test]
    #[ignore = "runs Vim, which `cargo test -- --ignored` takes from PATH"]
    fn the_editor_does_what_vim_does() {
        let mismatches: Vec<String> = VIM_CASES
            .iter()
            .filter_map(|&(text, keys)| {
                let (expected, actual) = (vim(text, keys), ours(text, keys));
                (expected != actual).then(|| {
                    format!("{keys:?} on {text:?}\n  vim:  {expected:?}\n  ours: {actual:?}")
                })
            })
            .collect();
        assert!(mismatches.is_empty(), "\n{}", mismatches.join("\n"));
    }
}
