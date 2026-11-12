//! Colors Markdown as it streams in, the way an editor highlights a Markdown file: every
//! character stays, so the text copies back as the Markdown it was.
//!
//! The colored text never depends on how the Markdown arrives in pieces: the colored pieces, one
//! after the other, are the colored whole. Each character is written once nothing after it can
//! change its color, and never changed after:
//! - most characters at once, since their color depends only on the characters before them;
//! - a line's start once it says what the line is, which the character after its mark settles,
//!   like the space in `- ` for a list item;
//! - a `*` with the next character, which makes it half of bold's `**` or leaves it a star;
//! - a code span once the backticks that close it end, since only a run of as many as opened it
//!   closes it, and at the line's end, the backticks of one that never closed, as plain text;
//! - a line colored whole at its end: a code fence, a line of a code block, which its grammar
//!   colors a line at a time, and a table's row, which waits for the table's end too, since the
//!   widest cell sets each column's width.

use crate::code;
use crate::table::Table;

use crate::style::{BOLD, CODE, DIM, RESET};

// Opens a code block, and closes it
const FENCE: &str = "```";

/// `text` as a code span, the way Markdown marks literal text: its backticks say it without
/// color, and the color says it at a glance.
pub fn code_span(text: &str) -> String {
    format!("{CODE}`{text}`{RESET}")
}

/// Colors one block of Markdown as it streams in.
pub struct Highlighter {
    code_block: Option<CodeBlock>,
    /// The rows of a table, held until the table ends.
    table: Vec<String>,
    line: Line,
}

/// A code block, colored in its language when a grammar knows the language.
enum CodeBlock {
    Plain,
    Language(code::Highlighter),
}

/// The line that streams in.
enum Line {
    /// The line's start, which does not settle yet what the line is.
    Start(String),
    Settled(Settled),
}

/// A line whose start settled what it is.
enum Settled {
    /// A table's row, colored with the table.
    Row(String),
    /// A code fence or a line of a code block.
    Code(String),
    /// A heading, all of it bold.
    Heading,
    /// Prose, with the inline marks open where it got to.
    Prose(Inline),
}

/// What a line is, which its start settles.
#[derive(Clone, Copy)]
enum Kind {
    Row,
    Code,
    Heading,
    /// Prose that starts with the mark of a list item or a quote, which takes this many bytes
    /// with the indent before it.
    Marked(usize),
    Prose,
}

impl Highlighter {
    pub fn new() -> Self {
        Highlighter {
            code_block: None,
            table: Vec::new(),
            line: Line::Start(String::new()),
        }
    }

    /// Takes the next piece of the Markdown and returns the colored text it settles, each line
    /// that ended followed by a line break.
    pub fn push(&mut self, text: &str) -> String {
        let mut colored = String::new();
        for character in text.chars() {
            self.push_character(character, &mut colored);
        }
        colored
    }

    /// Ends the line where the Markdown got to, when it got into one, and returns the colored
    /// text that settles.
    pub fn end_line(&mut self) -> String {
        match &self.line {
            Line::Start(start) if start.is_empty() => String::new(),
            Line::Start(_) | Line::Settled(_) => self.push("\n"),
        }
    }

    /// Ends the block and returns the colored text still held, then starts afresh for the next
    /// block.
    pub fn finish(&mut self) -> String {
        let mut colored = self.end_line();
        colored.push_str(&self.take_table());
        *self = Highlighter::new();
        colored
    }

    fn push_character(&mut self, character: char, colored: &mut String) {
        let is_end = character == '\n';
        match &mut self.line {
            Line::Start(start) => {
                if !is_end {
                    start.push(character);
                }
                let Some(kind) = kind(start, self.code_block.is_some(), is_end) else {
                    return;
                };
                let start = std::mem::take(start);
                self.line = Line::Settled(self.settle(kind, &start, colored));
                // The line's end settles what the line is, and then ends it
                if is_end {
                    self.push_character(character, colored);
                }
            }
            Line::Settled(line) if is_end => {
                match line {
                    Settled::Row(row) => self.table.push(std::mem::take(row)),
                    Settled::Code(code) => {
                        let code = std::mem::take(code);
                        colored.push_str(&self.color_code(&code));
                        colored.push('\n');
                    }
                    Settled::Heading => {
                        colored.push_str(RESET);
                        colored.push('\n');
                    }
                    Settled::Prose(inline) => {
                        std::mem::take(inline).end(colored);
                        colored.push('\n');
                    }
                }
                self.line = Line::Start(String::new());
            }
            Line::Settled(Settled::Row(text) | Settled::Code(text)) => text.push(character),
            Line::Settled(Settled::Heading) => colored.push(character),
            Line::Settled(Settled::Prose(inline)) => inline.push(character, colored),
        }
    }

    /// The line `kind` makes of its start, with the start's colored text: a line other than a
    /// row ends the table before it.
    fn settle(&mut self, kind: Kind, start: &str, colored: &mut String) -> Settled {
        match kind {
            Kind::Row => {}
            Kind::Code | Kind::Heading | Kind::Marked(_) | Kind::Prose => {
                colored.push_str(&self.take_table());
            }
        }
        match kind {
            Kind::Row => Settled::Row(start.to_string()),
            Kind::Code => Settled::Code(start.to_string()),
            Kind::Heading => {
                colored.push_str(BOLD);
                colored.push_str(start);
                Settled::Heading
            }
            Kind::Marked(length) => {
                let (mark, text) = start.split_at(length);
                colored.push_str(DIM);
                colored.push_str(mark);
                colored.push_str(RESET);
                let mut inline = Inline::default();
                inline.push_str(text, colored);
                Settled::Prose(inline)
            }
            Kind::Prose => {
                let mut inline = Inline::default();
                inline.push_str(start, colored);
                Settled::Prose(inline)
            }
        }
    }

    /// Colors a code fence, which opens or closes a code block, or a line of a code block.
    fn color_code(&mut self, line: &str) -> String {
        if is_fence(line) {
            self.code_block = if self.code_block.is_some() {
                None
            } else {
                let language = line
                    .trim()
                    .trim_start_matches('`')
                    .split_whitespace()
                    .next();
                match language.and_then(code::Highlighter::new) {
                    Some(highlighter) => Some(CodeBlock::Language(highlighter)),
                    None => Some(CodeBlock::Plain),
                }
            };
            // The whole fence line, language name included, is a mark
            return format!("{DIM}{line}{RESET}");
        }
        match &mut self.code_block {
            Some(CodeBlock::Language(highlighter)) => highlighter.color_line(line),
            // Only a fence opens a code block, so every line here but a fence is in one
            Some(CodeBlock::Plain) | None => format!("{CODE}{line}{RESET}"),
        }
    }

    /// The table's rows held, laid out when they make a table, and as they were written
    /// otherwise, each followed by a line break. A table wider than the terminal is laid out
    /// too, and the terminal wraps its lines.
    fn take_table(&mut self) -> String {
        let rows = std::mem::take(&mut self.table);
        let lines = match Table::read(&rows) {
            Some(table) => table.render(color_inline),
            None => rows,
        };
        lines.into_iter().map(|line| line + "\n").collect()
    }
}

/// What the line that starts with `start` is, once nothing after `start` can change it: when the
/// line is whole, or when the character after each mark arrived.
fn kind(start: &str, in_code_block: bool, is_whole: bool) -> Option<Kind> {
    if in_code_block {
        return Some(Kind::Code);
    }
    let rest = start.trim_start();
    if rest.is_empty() {
        return is_whole.then_some(Kind::Prose);
    }
    if rest.starts_with('|') {
        return Some(Kind::Row);
    }
    if is_fence(start) {
        return Some(Kind::Code);
    }
    // A start of the fence's backticks may grow into the fence
    if !is_whole && FENCE.starts_with(rest) {
        return None;
    }
    let text = start.trim_start_matches(' ');
    let indent = start.len() - text.len();
    // Whether the character after a mark of `length` bytes arrived, which settles the mark
    let is_settled = |length: usize| is_whole || length < text.len();
    let hashes = text.len() - text.trim_start_matches('#').len();
    if hashes > 0 {
        if !is_settled(hashes) {
            return None;
        }
        if text[hashes..].starts_with(' ') {
            return Some(Kind::Heading);
        }
    }
    let digits = text.len() - text.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    if digits > 0 && !is_settled(digits) {
        return None;
    }
    let mark_length = match text[digits..].chars().next() {
        Some('-' | '*' | '+' | '>') if digits == 0 => 1,
        Some('.' | ')') if digits > 0 => digits + 1,
        _ => 0,
    };
    if mark_length == 0 {
        return Some(Kind::Prose);
    }
    if !is_settled(mark_length) {
        return None;
    }
    // A mark is followed by a space, so `**bold**` or `-1` start no list item
    if text[mark_length..].starts_with(' ') {
        Some(Kind::Marked(indent + mark_length))
    } else {
        Some(Kind::Prose)
    }
}

fn is_fence(line: &str) -> bool {
    line.trim_start().starts_with(FENCE)
}

/// The inline marks open where a line got to: code spans in the code color, and bold in bold.
/// The marks stay, colored with what they mark.
#[derive(Default)]
struct Inline {
    /// A code span that waits for the backticks that close it.
    span: Option<Span>,
    is_bold: bool,
    /// A `*` that waits for the next character: with another `*` it opens or closes bold, and
    /// with any other character it is a star.
    has_star: bool,
}

/// A code span as Markdown's spec reads one: a run of backticks opens it, and only a run of as many
/// closes it, so a span can hold fewer or more of them.
/// Ref: <https://spec.commonmark.org/0.31.2/#code-spans>
#[derive(Default)]
struct Span {
    /// The backticks that open the span.
    ticks: usize,
    /// What follows them, the backticks that may close it included.
    text: String,
    /// The backticks in a row that end `text`.
    run: usize,
}

impl Span {
    fn is_closed(&self) -> bool {
        self.run == self.ticks
    }

    fn colored(&self) -> String {
        format!("{CODE}{}{}{RESET}", "`".repeat(self.ticks), self.text)
    }
}

impl Inline {
    fn push_str(&mut self, text: &str, colored: &mut String) {
        for character in text.chars() {
            self.push(character, colored);
        }
    }

    fn push(&mut self, character: char, colored: &mut String) {
        if let Some(span) = &mut self.span {
            match character {
                '`' if span.text.is_empty() => span.ticks += 1,
                '`' => {
                    span.text.push('`');
                    span.run += 1;
                }
                _ if span.is_closed() => {
                    colored.push_str(&span.colored());
                    if self.is_bold {
                        colored.push_str(BOLD);
                    }
                    self.span = None;
                    self.push(character, colored);
                }
                _ => {
                    span.text.push(character);
                    span.run = 0;
                }
            }
            return;
        }
        if std::mem::take(&mut self.has_star) {
            if character == '*' {
                if self.is_bold {
                    colored.push_str("**");
                    colored.push_str(RESET);
                } else {
                    colored.push_str(BOLD);
                    colored.push_str("**");
                }
                self.is_bold = !self.is_bold;
                return;
            }
            colored.push('*');
        }
        match character {
            '`' => {
                self.span = Some(Span {
                    ticks: 1,
                    ..Span::default()
                });
            }
            '*' => self.has_star = true,
            character => colored.push(character),
        }
    }

    /// Ends the line: a star that waits stays a star, a span that never closed leaves its
    /// backticks plain and its text read again, and the marks still open close.
    fn end(mut self, colored: &mut String) {
        if let Some(span) = self.span.take() {
            if span.is_closed() {
                colored.push_str(&span.colored());
            } else {
                colored.push_str(&"`".repeat(span.ticks));
                self.push_str(&span.text, colored);
                return self.end(colored);
            }
        }
        if self.has_star {
            colored.push('*');
        }
        if self.is_bold {
            colored.push_str(RESET);
        }
    }
}

/// Colors the inline marks of `text`, as a whole line's.
fn color_inline(text: &str) -> String {
    let mut colored = String::new();
    let mut inline = Inline::default();
    inline.push_str(text, &mut colored);
    inline.end(&mut colored);
    colored
}

#[cfg(test)]
mod tests {
    use super::*;

    fn color(lines: &[&str]) -> Vec<String> {
        color_in_pieces(&[lines.join("\n")])
            .lines()
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn plain_text_stays_as_it_is() {
        assert_eq!(color(&["Hello, world."]), ["Hello, world."]);
    }

    #[test]
    fn bold_keeps_its_marks_and_turns_bold() {
        assert_eq!(color(&["a **b** c"]), [format!("a {BOLD}**b**{RESET} c")]);
    }

    #[test]
    fn inline_code_turns_code_colored() {
        assert_eq!(
            color(&["run `ls` now"]),
            [format!("run {CODE}`ls`{RESET} now")]
        );
    }

    #[test]
    fn a_code_span_closes_only_with_as_many_backticks_as_opened_it() {
        assert_eq!(
            color(&["a `` `x` `` and `f``g` b"]),
            [format!(
                "a {CODE}`` `x` ``{RESET} and {CODE}`f``g`{RESET} b"
            )]
        );
    }

    #[test]
    fn backticks_that_never_close_stay_plain() {
        assert_eq!(color(&["a `b and ``"]), ["a `b and ``"]);
        // What the backticks held reads again as prose
        assert_eq!(color(&["`a **b**"]), [format!("`a {BOLD}**b**{RESET}")]);
    }

    #[test]
    fn a_heading_is_bold() {
        assert_eq!(
            color(&["## Title", "Text"]),
            [format!("{BOLD}## Title{RESET}"), "Text".to_string()]
        );
    }

    #[test]
    fn a_list_mark_is_dim() {
        assert_eq!(color(&["- item"]), [format!("{DIM}-{RESET} item")]);
        assert_eq!(color(&["  12. item"]), [format!("{DIM}  12.{RESET} item")]);
    }

    #[test]
    fn a_line_that_starts_with_bold_is_no_list_item() {
        assert_eq!(color(&["**b** c"]), [format!("{BOLD}**b**{RESET} c")]);
    }

    #[test]
    fn a_code_block_without_a_language_is_code_colored_and_ignores_marks() {
        assert_eq!(
            color(&["```", "# not a **heading**", "```"]),
            [
                format!("{DIM}```{RESET}"),
                format!("{CODE}# not a **heading**{RESET}"),
                format!("{DIM}```{RESET}"),
            ]
        );
    }

    #[test]
    fn a_code_block_in_a_known_language_is_colored_by_its_grammar() {
        let mut python = code::Highlighter::new("python").expect("syntect knows Python");
        assert_eq!(
            color(&["```python", "import os", "```"]),
            [
                format!("{DIM}```python{RESET}"),
                python.color_line("import os"),
                format!("{DIM}```{RESET}"),
            ]
        );
    }

    #[test]
    fn a_table_is_laid_out_once_it_ends() {
        let rows = ["| a | b |", "|---|---|", "| long cell | x |"];
        let lines: Vec<String> = rows.iter().map(ToString::to_string).collect();
        let mut expected = vec!["Before".to_string()];
        expected.extend(Table::read(&lines).expect("a table").render(color_inline));
        expected.push("After".to_string());
        assert_eq!(
            color(&["Before", rows[0], rows[1], rows[2], "After"]),
            expected
        );
    }

    #[test]
    fn a_table_that_ends_the_block_is_laid_out_when_the_block_ends() {
        let rows = ["| a |", "|---|", "| x |"];
        let lines: Vec<String> = rows.iter().map(ToString::to_string).collect();
        let laid_out = Table::read(&lines).expect("a table").render(color_inline);
        assert_eq!(color(&rows), laid_out);
    }

    #[test]
    fn rows_that_make_no_table_stay_as_written() {
        assert_eq!(
            color(&["| not a table |", "Text"]),
            ["| not a table |", "Text"]
        );
    }

    #[test]
    fn a_wide_table_is_laid_out_too() {
        let row = format!("| {} |", "x".repeat(200));
        let lines = vec![row.clone(), "|---|".to_string()];
        let laid_out = Table::read(&lines).expect("a table").render(color_inline);
        assert_eq!(color(&[&row, "|---|"]), laid_out);
    }

    #[test]
    fn a_lone_star_stays_plain() {
        assert_eq!(color(&["2 * 3 = 6"]), ["2 * 3 = 6"]);
    }

    /// Colors the Markdown that arrives in `pieces`, one after the other.
    fn color_in_pieces<S: AsRef<str>>(pieces: &[S]) -> String {
        let mut highlighter = Highlighter::new();
        let mut colored: String = pieces
            .iter()
            .map(|piece| highlighter.push(piece.as_ref()))
            .collect();
        colored.push_str(&highlighter.finish());
        colored
    }

    /// Markdown with each kind of line, mark and block the highlighter knows.
    const SAMPLE: &str = "## Title\n\nText with **bold**, `code`, 2 * 3 and ***.\n- item\n  12. item\n\
        12) item\n> quote\n-1 and #2\n**b** c\n   \n| a | b |\n|---|---|\n| `x` | **y** |\n\
        After\n```python\nimport os\n```\n  ```\n# not a heading\n```\n``x``\nends with *";

    #[test]
    fn the_colors_do_not_depend_on_how_the_text_arrives() {
        let whole = color_in_pieces(&[SAMPLE]);
        let characters: Vec<String> = SAMPLE.chars().map(String::from).collect();
        assert_eq!(color_in_pieces(&characters), whole);
        for (index, _) in SAMPLE.char_indices() {
            let (before, after) = SAMPLE.split_at(index);
            assert_eq!(
                color_in_pieces(&[before, after]),
                whole,
                "split at byte {index}"
            );
        }
    }

    /// Asserts that each piece, pushed in turn, settles the colored text paired with it.
    fn assert_settles(pieces: &[(&str, &str)]) {
        let mut highlighter = Highlighter::new();
        for (piece, settled) in pieces {
            assert_eq!(highlighter.push(piece), *settled, "after {piece:?}");
        }
    }

    #[test]
    fn prose_is_written_as_it_arrives() {
        assert_settles(&[("Hello wor", "Hello wor"), ("ld", "ld")]);
    }

    #[test]
    fn a_line_start_waits_for_the_character_that_settles_its_mark() {
        assert_settles(&[("-", ""), (" item", &format!("{DIM}-{RESET} item"))]);
        assert_settles(&[("-", ""), ("1", "-1")]);
        assert_settles(&[("12", ""), (".", ""), (" x", &format!("{DIM}12.{RESET} x"))]);
        assert_settles(&[("##", ""), (" T", &format!("{BOLD}## T"))]);
        assert_settles(&[("  ", ""), ("x", "  x")]);
        assert_settles(&[("``", ""), ("x", "")]);
    }

    #[test]
    fn a_star_waits_for_the_next_character() {
        assert_settles(&[
            ("a *", "a "),
            ("*", &format!("{BOLD}**")),
            ("b*", "b"),
            ("*", &format!("**{RESET}")),
        ]);
        assert_settles(&[("2 *", "2 "), (" 3", "* 3")]);
    }

    #[test]
    fn a_code_span_waits_for_the_backticks_that_close_it() {
        assert_settles(&[
            ("a `b", "a "),
            ("`", ""),
            (" c", &format!("{CODE}`b`{RESET} c")),
        ]);
    }

    #[test]
    fn a_fence_and_a_row_wait_for_their_line_to_end() {
        assert_settles(&[
            ("``", ""),
            ("`py", ""),
            ("\n", &format!("{DIM}```py{RESET}\n")),
        ]);
        assert_settles(&[("| a |", ""), ("\n", "")]);
    }
}
