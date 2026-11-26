//! Cutting what does not fit, across a row and down the rows, in one place, so every cut
//! follows the same rules, which the items below give with their sources. In the
//! conversation, only a noisy line is cut, as the README's Design section tells:
//! - a cut leaves a sign where it cuts: `…` at the end of a line, or a row that counts what it
//!   hides. Only empty rows go without one, since they hide nothing, and a half-typed vi
//!   command, as `label::newest` tells;
//! - a sign stands for text that a view shows whole, `Whole`. A row that counts names the key of
//!   that view, since it has the room, while an `…` names none, since the line it ends has none
//!   left, and stands for the full view, where every line the screen cuts shows whole;
//! - what is left takes the room that what was cut leaves.
//!
//! Ref: <https://v5-archive.patternfly.org/ux-writing/truncation>, which marks a cut with `…`

use std::num::NonZeroUsize;

use unicode_width::UnicodeWidthChar;

use crate::keys::{Field, Mark, Tone};
use crate::style::{SUBDUED, Style};
use crate::text::{Line, Row, Span};
use crate::wording::{Hint, with_hint};

/// A view that shows whole what a cut hides, which the cut's sign names, so no cut hides text
/// for good. The label row and the status are the exceptions, since a wider or a taller screen
/// shows them whole.
/// Ref: <https://www.w3.org/WAI/WCAG22/Understanding/reflow.html>, which allows a cut while a
/// mechanism reveals the text
/// Ref: <https://v5-archive.patternfly.org/ux-writing/truncation>, which asks for at least one
/// way to see the whole text
#[derive(Clone, Copy)]
pub enum Whole {
    /// The whole conversation, with every question and call whole: Ctrl+O.
    FullView,
}

impl Whole {
    /// The key that opens the view, as `keys.rs` binds it.
    fn key(self) -> &'static str {
        match self {
            Whole::FullView => "Ctrl+O",
        }
    }
}

/// A row drawn on the screen, which takes at most the screen's columns, since only a cut or
/// `empty` makes one, and so exactly one row of the terminal. A screen drawn from
/// these takes as many rows as it has rows, where a row that wrapped would take more: on the
/// question screen it would land on the row below, on a screen of its own it would push the rows
/// above it off the top, and among the main screen's live rows it could push them past the
/// screen's top, into the scrollback for good. An error stays whole, so its screen wraps it
/// onto rows before it counts them.
pub struct ScreenRow(Row);

impl ScreenRow {
    /// `line` as a row `columns` wide, cut at its end when longer, by `cut`.
    pub fn cut(line: &Line, columns: NonZeroUsize) -> Self {
        Self(Row::from(cut(line, columns)))
    }

    pub fn empty() -> Self {
        Self(Row::default())
    }

    /// The row on `background`, as `Row::on`, which takes no column.
    #[must_use]
    pub fn on(self, background: Style) -> Self {
        Self(self.0.on(background))
    }

    #[cfg(test)]
    pub fn line(&self) -> &Line {
        self.0.line()
    }

    pub fn to_terminal(&self) -> String {
        self.0.to_terminal()
    }
}

/// The row that counts `count` lines a cut hides of a text, which `whole` shows whole.
pub fn more_lines(count: usize, whole: Whole) -> String {
    let lines = if count == 1 { "line" } else { "lines" };
    let show = Hint::new(whole.key(), "shows it whole");
    with_hint(&format!("… {count} more {lines}"), &show)
}

/// The row that counts `count` fields of a call a cut hides, which `whole` shows whole.
pub fn more_fields(count: usize, whole: Whole) -> String {
    let (fields, them) = if count == 1 {
        ("field", "it")
    } else {
        ("fields", "them")
    };
    let does = format!("shows {them}");
    let show = Hint::new(whole.key(), &does);
    with_hint(&format!("… {count} more {fields}"), &show)
}

/// The row that swallows parts that are not all a call's fields, like a question's text, its
/// preview or the title, which it names as the request, which `whole` shows whole.
pub fn the_request(whole: Whole) -> String {
    let show = Hint::new(whole.key(), "shows the request");
    format!("… {show}")
}

/// `line` cut at its end to take at most `columns` columns on screen, with `…` where it is cut.
/// Text reads from its start, and an `…` at the end is always in the same place, where the eye
/// finds it, so every line is cut this way but a path's: `cut_path`.
/// Ref: <https://www.patternfly.org/components/truncate/design-guidelines>, whose default
/// truncation is at the end of the text
///
/// The `…` stands for the text it replaces, so it takes the style of that text's start.
pub fn cut(line: &Line, columns: NonZeroUsize) -> Line {
    if line.width() <= columns.get() {
        return line.clone();
    }
    let mut kept = Line::default();
    let mut width = 0;
    for (character, style) in line.characters() {
        let character_width = character.width().unwrap_or(0);
        if width + character_width > columns.get() - 1 {
            return kept.with(Span::new("…", style));
        }
        width += character_width;
        kept.push(Span::new(character, style));
    }
    unreachable!("a line wider than its columns has a character past them")
}

/// `path`, a file's path or a URL, cut in the middle to take at most `columns` columns on screen,
/// with `…` where it is cut. Paths often start the same, like `/home/…` or `https://`, and their
/// last part is the file or the page, so both the start and the end stay.
/// Ref: <https://www.patternfly.org/components/truncate/design-guidelines>, which cuts in the
/// middle a text whose end cannot be cut, like a long URL
/// Ref: <https://v5-archive.patternfly.org/ux-writing/truncation>, which cuts where the part
/// that tells one text from the next is not
///
/// The `…` takes the style of the start of the text it replaces, as for `cut`.
pub fn cut_path(path: &Line, columns: NonZeroUsize) -> Line {
    if path.width() <= columns.get() {
        return path.clone();
    }
    let characters: Vec<(char, Style)> = path.characters().collect();
    // The `…` takes a column, and the start takes the odd one of the rest
    let kept_columns = columns.get() - 1;
    let fits = |room: usize| {
        let mut width = 0;
        move |(character, _): &&(char, Style)| {
            width += character.width().unwrap_or(0);
            width <= room
        }
    };
    let start = (characters.iter())
        .take_while(fits(kept_columns.div_ceil(2)))
        .count();
    let end = (characters.iter().rev())
        .take_while(fits(kept_columns / 2))
        .count();
    let (_, cut_style) = characters[start];
    (characters[..start].iter().copied())
        .chain([('…', cut_style)])
        .chain(characters[characters.len() - end..].iter().copied())
        .collect()
}

/// `line` cut to `columns` columns with `…` at its end even when it fits, since what follows it
/// is hidden.
pub fn cut_before_hidden(line: &Line, columns: NonZeroUsize) -> Line {
    if line.width() < columns.get() {
        line.clone().with("…")
    } else {
        // A space after it makes the line too wide, so the cut always takes place
        cut(&line.clone().with(" "), columns)
    }
}

/// `lines` fitted in `rows` rows, cut down from the end, as a list reads from its start, as `fit`
/// fits fields: first the empty lines go, the last first, which hide nothing, then the last
/// lines, swallowed by the last row, which counts them. A taller screen shows them, as a wider
/// screen shows the label row whole, so the row names no key.
pub fn fit_lines(lines: &[Line], rows: usize) -> Vec<Line> {
    let mut lines = lines.to_vec();
    while lines.len() > rows {
        let Some(index) = lines.iter().rposition(Line::is_empty) else {
            break;
        };
        lines.remove(index);
    }
    if lines.len() <= rows || rows == 0 {
        lines.truncate(rows);
        return lines;
    }
    let hidden = lines.len() - (rows - 1);
    lines.truncate(rows - 1);
    lines.push(Line::from(Span::new(
        format!("… {hidden} more rows"),
        SUBDUED,
    )));
    lines
}

/// The rows a field needs to show anything: none for empty rows, which hide nothing, and one for
/// any other, a preview too, whose one row ends in `…`.
fn min_rows(field: &Field) -> usize {
    match field.mark {
        Mark::None if field.value.is_empty() => 0,
        _ => 1,
    }
}

/// `fields` fitted in `rows` rows, cut down from the end, as a list reads from its start. First
/// the rows a field leaves empty go, which hide nothing: the last field with more rows than its
/// value takes, by `used_rows`, gives one up. Then the last field that can gives up a row of its
/// value, until each shows only what it must, and then the last fields are hidden, swallowed by
/// the last row, which names them: as fields when they all are a call's, and else as the request.
/// Every field hidden, and every row a field gives up, shows whole in the full view.
///
/// The last row is the one sign of the whole stack: a shorter screen only makes it swallow more
/// and name what it swallows more broadly, from a call's fields to the request, and on a stack of
/// one row, the title too. So the user learns one row, always at the end, always
/// naming the key that shows it all.
/// Ref: <https://primer.style/product/components/underline-nav>, whose overflow menu takes the
/// items that do not fit one by one
/// Ref: <https://css-tricks.com/the-priority-navigation-pattern/>, the Priority+ pattern, whose
/// "more" holds what the screen has no room for
/// Ref: <https://www.patternfly.org/components/overflow-menu/design-guidelines>, whose actions
/// collapse into one menu as the screen shrinks
pub fn fit(fields: &[Field], rows: usize, used_rows: impl Fn(&Field) -> usize) -> Vec<Field> {
    if rows == 0 {
        return Vec::new();
    }
    let total = |fields: &[Field]| fields.iter().map(|field| field.rows).sum::<usize>();
    let mut fields = fields.to_vec();
    let floors: [&dyn Fn(&Field) -> usize; 2] = [
        &|field: &Field| used_rows(field).max(min_rows(field)),
        &min_rows,
    ];
    for floor in floors {
        while total(&fields) > rows {
            let Some(index) = fields.iter().rposition(|field| field.rows > floor(field)) else {
                break;
            };
            fields[index].rows -= 1;
            if fields[index].rows == 0 {
                fields.remove(index);
            }
        }
    }
    if total(&fields) <= rows {
        return fields;
    }
    // The fields that fit with the row that swallows the others
    let mut kept = Vec::new();
    let mut taken = 1;
    for field in &fields {
        if taken + field.rows > rows {
            break;
        }
        taken += field.rows;
        kept.push(field.clone());
    }
    let hidden = &fields[kept.len()..];
    let is_call =
        (hidden.iter()).all(|field| matches!(field.mark, Mark::Sign { .. } | Mark::Name(_)));
    kept.push(Field {
        mark: Mark::None,
        value: if is_call {
            more_fields(hidden.len(), Whole::FullView)
        } else {
            the_request(Whole::FullView)
        },
        is_path: false,
        tone: Tone::Note,
        rows: 1,
    });
    kept
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::{STRONG, SUBDUED};

    fn columns(count: usize) -> NonZeroUsize {
        NonZeroUsize::new(count).expect("the count is not zero")
    }

    fn plain(text: &str) -> Line {
        Line::from(text)
    }

    fn subdued(text: &str) -> Span {
        Span::new(text, SUBDUED)
    }

    #[test]
    fn lines_give_up_their_empty_rows_first_from_the_end() {
        let lines = [plain("a"), plain(""), plain("b"), plain(""), plain("c")];
        let texts = |lines: Vec<Line>| lines.iter().map(Line::text).collect::<Vec<_>>();
        assert_eq!(texts(fit_lines(&lines, 4)), ["a", "", "b", "c"]);
        assert_eq!(texts(fit_lines(&lines, 3)), ["a", "b", "c"]);
        assert_eq!(texts(fit_lines(&lines, 2)), ["a", "… 2 more rows"]);
        assert_eq!(texts(fit_lines(&lines, 0)), Vec::<String>::new());
    }

    #[test]
    fn a_line_that_fits_stays_whole() {
        assert_eq!(cut_path(&plain("abc"), columns(3)), plain("abc"));
    }

    #[test]
    fn a_path_too_long_keeps_its_start_and_its_end() {
        assert_eq!(cut_path(&plain("abcdefgh"), columns(6)), plain("abc…gh"));
    }

    #[test]
    fn the_end_kept_keeps_its_style() {
        assert_eq!(
            cut_path(&Line::from(subdued("abcdef")), columns(4)),
            Line::from(subdued("ab…f"))
        );
        let path = Line::from([Span::new("ab", STRONG), "cdefgh".into()]);
        assert_eq!(
            cut_path(&path, columns(4)),
            Line::from([Span::new("ab", STRONG), "…h".into()])
        );
    }

    #[test]
    fn the_ellipsis_takes_the_style_of_the_text_it_replaces() {
        let path = Line::from(["ab".into(), subdued("cdefgh")]);
        assert_eq!(
            cut_path(&path, columns(4)),
            Line::from(["ab".into(), subdued("…h")])
        );
        // The style set before the cut holds after it, where once its code was lost
        let line = Line::from([Span::new("name", STRONG), ": value".into()]);
        assert_eq!(
            cut(&line, columns(3)),
            Line::from([Span::new("na", STRONG), Span::new("…", STRONG)])
        );
    }

    #[test]
    fn a_line_too_long_keeps_its_start_and_its_style() {
        assert_eq!(cut(&plain("abcdef"), columns(6)), plain("abcdef"));
        assert_eq!(cut(&plain("abcdefgh"), columns(6)), plain("abcde…"));
        assert_eq!(
            cut(&Line::from(subdued("abcdef")), columns(4)),
            Line::from(subdued("abc…"))
        );
        assert_eq!(cut(&plain("ab日本"), columns(4)), plain("ab…"));
    }

    #[test]
    fn a_wide_character_that_does_not_fit_is_cut_whole() {
        assert_eq!(cut_path(&plain("ab日本"), columns(4)), plain("ab…"));
    }

    #[test]
    fn a_line_before_hidden_text_always_ends_in_an_ellipsis() {
        assert_eq!(cut_before_hidden(&plain("ab"), columns(4)), plain("ab…"));
        assert_eq!(cut_before_hidden(&plain("abcd"), columns(4)), plain("abc…"));
        assert_eq!(
            cut_before_hidden(&plain("abcdef"), columns(4)),
            plain("abc…")
        );
    }

    fn field(value: &str, rows: usize) -> Field {
        Field {
            mark: Mark::None,
            value: value.to_string(),
            is_path: false,
            tone: Tone::Plain,
            rows,
        }
    }

    /// The values and rows of fitted fields.
    fn shape(fields: &[Field]) -> Vec<(&str, usize)> {
        (fields.iter())
            .map(|field| (field.value.as_str(), field.rows))
            .collect()
    }

    /// A field of a call, which a name marks.
    fn call_field(value: &str, rows: usize) -> Field {
        let mark = if value.is_empty() {
            Mark::None
        } else {
            Mark::Name("name".to_string())
        };
        Field {
            mark,
            ..field(value, rows)
        }
    }

    #[test]
    fn fields_give_up_rows_from_the_end_then_the_last_row_swallows_them() {
        let fields = [
            call_field("a", 3),
            call_field("", 2),
            call_field("b", 1),
            call_field("c", 1),
        ];
        let fitted = fit(&fields, 7, |_| 0);
        assert_eq!(shape(&fitted), [("a", 3), ("", 2), ("b", 1), ("c", 1)]);
        // Empty rows go first, whole, then the last field of more rows gives up its rows
        let fitted = fit(&fields, 4, |_| 0);
        assert_eq!(shape(&fitted), [("a", 2), ("b", 1), ("c", 1)]);
        let fitted = fit(&fields, 3, |_| 0);
        assert_eq!(shape(&fitted), [("a", 1), ("b", 1), ("c", 1)]);
        let fitted = fit(&fields, 2, |_| 0);
        assert_eq!(
            shape(&fitted),
            [("a", 1), ("… 2 more fields · Ctrl+O shows them", 1)]
        );
        // On one row, the row swallows every field
        let fitted = fit(&fields, 1, |_| 0);
        assert_eq!(shape(&fitted), [("… 3 more fields · Ctrl+O shows them", 1)]);
        assert_eq!(shape(&fit(&fields, 0, |_| 0)), []);
    }

    #[test]
    fn the_last_row_names_parts_that_are_not_a_calls_fields_as_the_request() {
        let fitted = fit(&[field("question", 1), preview("x", 1)], 1, |_| 0);
        assert_eq!(shape(&fitted), [("… Ctrl+O shows the request", 1)]);
    }

    #[test]
    fn a_preview_gives_up_its_rows_down_to_one_as_any_value() {
        let fields = [field("question", 3), preview("x\ny\nz", 5)];
        let rows = |count| {
            let fitted = fit(&fields, count, |_| 0);
            fitted.iter().map(|field| field.rows).collect::<Vec<_>>()
        };
        assert_eq!(rows(8), [3, 5]);
        assert_eq!(rows(5), [3, 2]);
        assert_eq!(rows(4), [3, 1]);
        assert_eq!(rows(3), [2, 1]);
        assert_eq!(rows(2), [1, 1]);
        assert_eq!(rows(1), [1]);
        // The question's rows its text leaves empty go before the preview's
        let used_rows = |field: &Field| field.value.lines().count();
        let fitted = fit(&fields, 5, used_rows);
        assert_eq!(shape(&fitted), [("question", 2), ("x\ny\nz", 3)]);
    }

    /// The preview of an option, of `markdown`, in `rows` rows.
    fn preview(markdown: &str, rows: usize) -> Field {
        Field {
            mark: Mark::Quote(Some("1".to_string())),
            ..field(markdown, rows)
        }
    }
}
