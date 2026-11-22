//! Styled text: pieces of text each in one style, kept apart from the codes that show the style
//! until the text reaches the terminal or the pager. A piece's style is whole in itself, so
//! cutting, measuring or joining text keeps every character's style, with no codes to read
//! back.
//! Ref: <https://docs.rs/ratatui/latest/ratatui/text/index.html>, which models text as spans
//! and lines the same way

use std::fmt::{self, Display, Formatter};
use std::mem;

use serde::Deserialize;
use unicode_width::UnicodeWidthStr;

use crate::style::{PLAIN, Style};

/// Erases to the end of the row in the background that is set, which fills the row's rest with it
/// without writing spaces into the text.
const FILL_ROW: &str = "\x1b[K";

/// Asks the terminal to wrap the cursor's line at spaces, as the README's Wrapping at spaces tells.
const WRAP_AT_SPACES: &str = "\x1b]2029\x1b\\";

/// Where the terminal breaks a line too wide for the screen.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Wrap {
    /// At the screen's edge, as every terminal does.
    #[default]
    Edge,
    /// At the last space before the edge, in a terminal that knows `OSC 2029`, and at the edge in
    /// any other, which ignores it. The config names it `ask-spaces`, since claudio only asks.
    #[serde(rename = "ask-spaces")]
    Spaces,
}

/// A piece of text in one style. The text holds no control character, like a line break or an
/// escape, so each character takes the columns its width says: claudio splits lines at their
/// breaks, shows a tab as a space, and takes the escapes out of what Claude Code sends. A control
/// character that gets here anyway moves the cursor or changes colors where it shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    text: String,
    style: Style,
}

impl Span {
    pub fn new(text: impl Into<String>, style: Style) -> Self {
        Span {
            text: text.into(),
            style,
        }
    }
}

impl From<&str> for Span {
    fn from(text: &str) -> Self {
        Span::new(text, PLAIN)
    }
}

impl From<String> for Span {
    fn from(text: String) -> Self {
        Span::new(text, PLAIN)
    }
}

/// A line of styled text, with no line break.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Line {
    /// Neighbors differ in style and none is empty, so a line has one form, which equality
    /// compares.
    spans: Vec<Span>,
}

impl Line {
    /// Adds `span` at the end, joined to the last span when they share a style.
    pub fn push(&mut self, span: impl Into<Span>) {
        let span = span.into();
        if span.text.is_empty() {
            return;
        }
        match self.spans.last_mut() {
            Some(last) if last.style == span.style => last.text.push_str(&span.text),
            _ => self.spans.push(span),
        }
    }

    /// Adds `line`'s spans at the end, in their styles.
    pub fn append(&mut self, line: Line) {
        for span in line.spans {
            self.push(span);
        }
    }

    /// The line with `span` at the end.
    #[must_use]
    pub fn with(mut self, span: impl Into<Span>) -> Self {
        self.push(span);
        self
    }

    /// The line with `line`'s spans at the end.
    #[must_use]
    pub fn with_line(mut self, line: Line) -> Self {
        self.append(line);
        self
    }

    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    /// The columns the line takes on screen.
    pub fn width(&self) -> usize {
        self.spans.iter().map(|span| span.text.width()).sum()
    }

    /// The line's characters, each with its style, which `collect` makes a line of again.
    pub fn characters(&self) -> impl DoubleEndedIterator<Item = (char, Style)> + '_ {
        (self.spans.iter())
            .flat_map(|span| span.text.chars().map(|character| (character, span.style)))
    }

    /// The line's text, without its styles, as it copies.
    pub fn text(&self) -> String {
        self.spans.iter().map(|span| span.text.as_str()).collect()
    }

    /// The line on `background`, as `Row::on` puts it.
    pub fn on(self, background: Style) -> Row {
        Row::from(self).on(background)
    }
}

/// The line with the code of each span's style in front of it, and a reset at its end, so its
/// styles reach nothing written after it.
impl Display for Line {
    fn fmt(&self, formatter: &mut Formatter) -> fmt::Result {
        let mut style = PLAIN;
        for span in &self.spans {
            if span.style != style {
                formatter.write_str(&span.style.code())?;
                style = span.style;
            }
            formatter.write_str(&span.text)?;
        }
        if style != PLAIN {
            formatter.write_str(&PLAIN.code())?;
        }
        Ok(())
    }
}

/// A whole line on its row, with the style the row takes past the line's end, as on the bar, none
/// for the terminal's own. Nothing joins a row after its end, so the end keeps its background.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Row {
    line: Line,
    background: Option<Style>,
    wrap: Wrap,
}

impl Row {
    pub fn line(&self) -> &Line {
        &self.line
    }

    /// The row's line, without the style of the row past its end, as text that sets no
    /// background, like Markdown, reads.
    pub fn into_line(self) -> Line {
        self.line
    }

    /// The row on `background`: each span takes its colors where it has none, and so does the
    /// rest of the row past the line's end, as a style keeps its own colors on another.
    #[must_use]
    pub fn on(self, background: Style) -> Self {
        let line = (self.line.spans.into_iter())
            .map(|span| Span::new(span.text, span.style.on(background)))
            .collect();
        Row {
            line,
            background: self.background.or(Some(background)),
            wrap: self.wrap,
        }
    }

    /// The row as the terminal shows it, its background filling the row before the line and after
    /// it, so it fills the first row and the last when the terminal wraps the line.
    pub fn to_terminal(&self) -> String {
        let wrap = match self.wrap {
            Wrap::Edge => "",
            Wrap::Spaces => WRAP_AT_SPACES,
        };
        let Some(background) = self.background else {
            return format!("{wrap}{}", self.line);
        };
        let fill = format!("{}{FILL_ROW}", background.code());
        format!("{wrap}{fill}{}{fill}{}", self.line, PLAIN.code())
    }

    #[cfg(test)]
    pub fn wrap(&self) -> Wrap {
        self.wrap
    }

    /// The row as a pager shows it, which reads only the codes of colors, so the row's rest
    /// stays the terminal's own.
    pub fn to_pager(&self) -> String {
        self.line.to_string()
    }
}

impl From<Line> for Row {
    fn from(line: Line) -> Self {
        Row {
            line,
            background: None,
            wrap: Wrap::Edge,
        }
    }
}

impl From<Span> for Line {
    fn from(span: Span) -> Self {
        Line::default().with(span)
    }
}

impl From<&str> for Line {
    fn from(text: &str) -> Self {
        Line::from(Span::from(text))
    }
}

impl From<String> for Line {
    fn from(text: String) -> Self {
        Line::from(Span::from(text))
    }
}

impl<const N: usize> From<[Span; N]> for Line {
    fn from(spans: [Span; N]) -> Self {
        spans.into_iter().collect()
    }
}

impl FromIterator<Span> for Line {
    fn from_iter<I: IntoIterator<Item = Span>>(spans: I) -> Self {
        let mut line = Line::default();
        for span in spans {
            line.push(span);
        }
        line
    }
}

impl FromIterator<(char, Style)> for Line {
    fn from_iter<I: IntoIterator<Item = (char, Style)>>(characters: I) -> Self {
        let mut line = Line::default();
        for (character, style) in characters {
            line.push(Span::new(character, style));
        }
        line
    }
}

/// Text as it streams to the terminal: the lines that ended, each to be followed by a line break,
/// then the line still open. Text written in pieces is the text written whole, since `append`
/// joins the open line of one piece to the first line of the next.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Text {
    ended: Vec<Row>,
    open: Line,
}

impl Text {
    /// Adds `span` to the open line.
    pub fn push(&mut self, span: impl Into<Span>) {
        self.open.push(span);
    }

    /// Ends the open line as a row the terminal wraps at `wrap`, and opens an empty one.
    pub fn end_line(&mut self, wrap: Wrap) {
        let line = mem::take(&mut self.open);
        self.ended.push(Row {
            wrap,
            ..Row::from(line)
        });
    }

    /// Ends the open line with `row` after it, which ends the row and so keeps its background.
    pub fn push_line(&mut self, row: impl Into<Row>) {
        let row = row.into();
        let mut line = mem::take(&mut self.open);
        line.append(row.line);
        self.ended.push(Row { line, ..row });
    }

    /// Adds `text` at the end: its first line continues the open line.
    pub fn append(&mut self, text: Text) {
        for line in text.ended {
            self.push_line(line);
        }
        self.open.append(text.open);
    }

    pub fn is_empty(&self) -> bool {
        self.ended.is_empty() && self.open.is_empty()
    }

    /// The rows that ended, which leave the text.
    pub fn take_ended(&mut self) -> Vec<Row> {
        mem::take(&mut self.ended)
    }

    pub fn open(&self) -> &Line {
        &self.open
    }

    /// The ended rows, then the open line when it holds text.
    pub fn into_rows(self) -> Vec<Row> {
        let mut rows = self.ended;
        if !self.open.is_empty() {
            rows.push(Row::from(self.open));
        }
        rows
    }

    /// The text as the pager shows it, each ended row followed by a line break.
    pub fn to_pager(&self) -> String {
        let mut written = String::new();
        for row in &self.ended {
            written.push_str(&row.to_pager());
            written.push('\n');
        }
        written.push_str(&self.open.to_string());
        written
    }

    /// The text as the terminal shows it, each ended row followed by `line_break`.
    pub fn to_terminal(&self, line_break: &str) -> String {
        let mut written = String::new();
        for row in &self.ended {
            written.push_str(&row.to_terminal());
            written.push_str(line_break);
        }
        written.push_str(&self.open.to_string());
        written
    }
}

#[cfg(test)]
impl Line {
    /// A line of `spans`, each a text in its style.
    pub fn styled<const N: usize>(spans: [(&str, Style); N]) -> Self {
        (spans.into_iter())
            .map(|(text, style)| Span::new(text, style))
            .collect()
    }

    /// The style of the first `text` in the line, when all of it takes one.
    pub fn style_of(&self, text: &str) -> Option<Style> {
        let line = self.text();
        let start = line[..line.find(text)?].chars().count();
        let mut styles =
            (self.characters().skip(start).take(text.chars().count())).map(|(_, style)| style);
        let first = styles.next()?;
        styles.all(|style| style == first).then_some(first)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::{STRONG, SUBDUED, YOURS};

    #[test]
    fn spans_of_one_style_join_and_empty_ones_go() {
        let line = Line::from([
            Span::new("a", STRONG),
            Span::new("", SUBDUED),
            Span::new("b", STRONG),
        ]);
        assert_eq!(line, Line::from(Span::new("ab", STRONG)));
    }

    #[test]
    fn a_line_writes_each_style_whole_and_resets_at_its_end() {
        let line = Line::from([Span::new("a", STRONG), "b".into()]);
        assert_eq!(line.to_string(), "\x1b[0;1ma\x1b[0mb");
        assert_eq!(Line::from("a").to_string(), "a");
    }

    #[test]
    fn spans_that_a_background_makes_alike_join() {
        let line = Line::from(["a".into(), Span::new("b", YOURS)]).on(YOURS);
        assert_eq!(line, Line::from(Span::new("ab", YOURS)).on(YOURS));
    }

    #[test]
    fn only_a_row_that_wraps_at_spaces_asks_the_terminal_to() {
        let mut text = Text::default();
        text.push("a b");
        text.end_line(Wrap::Spaces);
        text.push("a b");
        text.end_line(Wrap::Edge);
        let rows = text.take_ended();
        assert_eq!(rows[0].to_terminal(), "\x1b]2029\x1b\\a b");
        assert_eq!(rows[0].to_pager(), "a b");
        assert_eq!(rows[1].to_terminal(), "a b");
    }

    #[test]
    fn a_row_keeps_its_own_background_on_another() {
        let row = Line::from("a").on(YOURS).on(STRONG);
        assert_eq!(row.background, Some(YOURS));
    }

    #[test]
    fn text_before_a_row_leaves_the_row_its_background() {
        let mut text = Text::default();
        text.push("a");
        text.push_line(Line::from("b").on(YOURS));
        let row = Line::from(["a".into(), Span::new("b", YOURS)]);
        assert_eq!(
            text.take_ended(),
            [Row {
                line: row,
                background: Some(YOURS),
                wrap: Wrap::Edge,
            }]
        );
    }

    #[test]
    fn a_line_on_a_background_fills_its_row() {
        let line = Line::from("a").on(YOURS);
        let bar = YOURS.code();
        assert_eq!(
            line.to_terminal(),
            format!("{bar}{FILL_ROW}{bar}a\x1b[0m{bar}{FILL_ROW}\x1b[0m")
        );
    }

    #[test]
    fn text_written_in_pieces_is_the_text_written_whole() {
        let mut pieces = Text::default();
        let mut first = Text::default();
        first.push("a");
        first.end_line(Wrap::Edge);
        first.push("b");
        let mut second = Text::default();
        second.push("c");
        second.end_line(Wrap::Edge);
        pieces.append(first);
        pieces.append(second);
        let mut whole = Text::default();
        whole.push_line(Line::from("a"));
        whole.push_line(Line::from("bc"));
        assert_eq!(pieces, whole);
    }
}
