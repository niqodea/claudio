//! Cutting what does not fit, across a row and down the rows, in one place, so every cut
//! follows the same rules, which AGENTS.md's Cutting section gives with their sources:
//! - a cut leaves a sign where it cuts: `…` at the end of a line, or a row that counts what it
//!   hides. Only empty rows go without one, since they hide nothing;
//! - a sign stands for text that a view shows whole, `Whole`. A row that counts names the key of
//!   that view, since it has the room, while an `…` names none, since the line it ends has none
//!   left, and stands for the full view, where every line the screen cuts shows whole;
//! - what is left takes the room that what was cut leaves.

use std::num::NonZeroUsize;

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::keys::{Field, Mark, Tone};
use crate::style::RESET;

/// A view that shows whole what a cut hides, which the cut's sign names.
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

/// A row of the question screen, which takes at most the screen's columns, since only a cut,
/// `spread` or `empty` makes one. A row there must take exactly one row of the terminal: one that wrapped
/// would land on the row below it, so the screen draws only rows of this type.
pub struct ScreenRow(String);

impl ScreenRow {
    /// `line` as a row `columns` wide, cut at its end when longer, by `cut`.
    pub fn cut(line: &str, columns: NonZeroUsize) -> Self {
        Self(cut(line, columns))
    }

    pub fn empty() -> Self {
        Self(String::new())
    }

    /// A row `columns` wide with `left` at its start and one of `right`'s versions, from whole
    /// to shortest, at its end, at least a space apart. The right gives up text first, taking the
    /// first version that fits beside the whole `left`, then `left` gives up its end, beside the
    /// shortest version, so every `…` falls between them. When not even that fits, the row is
    /// `left` alone, cut.
    /// Ref: <https://ec.europa.eu/component-library/v3.13.0/ec/components/navigation/breadcrumb/code/>,
    /// whose breadcrumbs keep their first item and their last ones and hide the middle ones
    /// behind `…`
    pub fn spread(left: &str, right: &[String], columns: NonZeroUsize) -> Self {
        let fits_beside = |right: &&String, left_width: usize| {
            left_width + 1 + visible_width(right) <= columns.get()
        };
        let Some(right) = (right.iter())
            .find(|right| fits_beside(right, visible_width(left)))
            .or(right.last())
            .filter(|right| fits_beside(right, 1))
        else {
            return Self::cut(left, columns);
        };
        let room = NonZeroUsize::new(columns.get() - visible_width(right) - 1)
            .expect("the version found leaves a column for the left");
        let left = cut(left, room);
        let gap = columns.get() - visible_width(&left) - visible_width(right);
        Self(format!("{left}{}{right}", " ".repeat(gap)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The row that counts `count` lines a cut hides of a text, which `whole` shows whole.
pub fn more_lines(count: usize, whole: Whole) -> String {
    let lines = if count == 1 { "line" } else { "lines" };
    format!("… {count} more {lines} · {} shows it whole", whole.key())
}

/// The row that counts `count` fields of a call a cut hides, which `whole` shows whole.
pub fn more_fields(count: usize, whole: Whole) -> String {
    let (fields, them) = if count == 1 {
        ("field", "it")
    } else {
        ("fields", "them")
    };
    format!("… {count} more {fields} · {} shows {them}", whole.key())
}

/// The row that swallows parts that are not all a call's fields, like a question's text, its
/// preview or the title, which it names as the request, which `whole` shows whole.
pub fn the_request(whole: Whole) -> String {
    format!("… {} shows the request", whole.key())
}

/// `line` cut at its end to take at most `columns` columns on screen, with `…` where it is cut.
/// Text reads from its start, and an `…` at the end is always in the same place, where the eye
/// finds it, so every line is cut this way but a path's: `cut_path`.
/// Ref: <https://www.patternfly.org/components/truncate/design-guidelines>, whose default
/// truncation is at the end of the text
///
/// The line holds its own colors, as for `cut_path`.
pub fn cut(line: &str, columns: NonZeroUsize) -> String {
    let mut width = 0;
    let mut rest = line.char_indices();
    let mut start_end = None;
    let kept_columns = columns.get() - 1;
    while let Some((index, character)) = rest.next() {
        if character == '\x1b' {
            // Every sequence a line holds is a CSI, which ends with a letter and takes no column
            rest.find(|(_, next)| next.is_ascii_alphabetic());
            continue;
        }
        let character_width = character.width().unwrap_or(0);
        if start_end.is_none() && width + character_width > kept_columns {
            start_end = Some(index);
        }
        width += character_width;
    }
    match start_end {
        Some(start_end) if width > columns.get() => {
            // The `…` stands for the text it replaces, so it takes the colors of that text
            format!(
                "{}{RESET}{}…{RESET}",
                &line[..start_end],
                colors_at(line, start_end),
            )
        }
        _ => line.to_string(),
    }
}

/// `path`, a file's path or a URL, cut in the middle to take at most `columns` columns on screen,
/// with `…` where it is cut. Paths often start the same, like `/home/…` or `https://`, and their
/// last part is the file or the page, so both the start and the end stay.
/// Ref: <https://www.patternfly.org/components/truncate/design-guidelines>, which cuts in the
/// middle a text whose end cannot be cut, like a long URL
/// Ref: <https://v5-archive.patternfly.org/ux-writing/truncation>, which cuts where the part
/// that tells one text from the next is not
///
/// The path holds its own colors, since the `…` and the end take the colors in effect where the
/// path has them, and know nothing of colors set before it.
pub fn cut_path(path: &str, columns: NonZeroUsize) -> String {
    let mut characters = Vec::new();
    let mut rest = path.char_indices();
    while let Some((index, character)) = rest.next() {
        if character == '\x1b' {
            // Every sequence a path holds is a CSI, which ends with a letter and takes no column
            rest.find(|(_, next)| next.is_ascii_alphabetic());
            continue;
        }
        characters.push((index, character, character.width().unwrap_or(0)));
    }
    if characters.iter().map(|(_, _, width)| width).sum::<usize>() <= columns.get() {
        return path.to_string();
    }
    // The `…` takes a column, and the start takes the odd one of the rest
    let kept_columns = columns.get() - 1;
    let (mut start_end, mut start_width) = (0, 0);
    for (index, character, width) in &characters {
        if start_width + width > kept_columns.div_ceil(2) {
            break;
        }
        start_width += width;
        start_end = index + character.len_utf8();
    }
    let (mut end_start, mut end_width) = (path.len(), 0);
    for (index, _, width) in characters.iter().rev() {
        if end_width + width > kept_columns / 2 {
            break;
        }
        end_width += width;
        end_start = *index;
    }
    // The `…` stands for the text it replaces, so it takes the colors of that text's start
    let cut_start = characters
        .iter()
        .find(|(index, _, _)| *index >= start_end)
        .map_or(start_end, |(index, _, _)| *index);
    let end = &path[end_start..];
    let end = if end.is_empty() {
        String::new()
    } else {
        format!("{}{end}", colors_at(path, end_start))
    };
    format!(
        "{}{RESET}{}…{RESET}{end}",
        &path[..start_end],
        colors_at(path, cut_start),
    )
}

/// The color codes in effect at byte `end` of `line`, those since its last reset, which set the
/// colors again where printing starts there.
pub fn colors_at(line: &str, end: usize) -> String {
    let mut codes = String::new();
    let mut characters = line[..end].char_indices();
    while let Some((index, character)) = characters.next() {
        if character != '\x1b' {
            continue;
        }
        // Every sequence a line holds is a CSI, which ends with a letter, and those that end with
        // `m` set colors
        let sequence_end = characters
            .find(|(_, next)| next.is_ascii_alphabetic())
            .map_or(end, |(next_index, next)| next_index + next.len_utf8());
        let sequence = &line[index..sequence_end];
        if sequence == RESET {
            codes.clear();
        } else if sequence.ends_with('m') {
            codes.push_str(sequence);
        }
    }
    codes
}

/// The columns `text` takes on screen, without its escape sequences.
pub fn visible_width(text: &str) -> usize {
    let mut visible = String::new();
    let mut characters = text.chars();
    while let Some(character) = characters.next() {
        if character == '\x1b' {
            // Every sequence a line holds is a CSI, which ends with a letter
            for next in characters.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            visible.push(character);
        }
    }
    visible.width()
}

/// `line` cut to `columns` columns with `…` at its end even when it fits, since what follows it
/// is hidden.
pub fn cut_before_hidden(line: &str, columns: NonZeroUsize) -> String {
    if visible_width(line) < columns.get() {
        format!("{line}{RESET}…{RESET}")
    } else {
        // A space after it makes the line too wide, so the cut always takes place
        cut(&format!("{line} "), columns)
    }
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

    #[test]
    fn a_line_that_fits_stays_whole() {
        assert_eq!(cut_path("abc", columns(3)), "abc");
    }

    #[test]
    fn a_path_too_long_keeps_its_start_and_its_end() {
        assert_eq!(
            cut_path("abcdefgh", columns(6)),
            format!("abc{RESET}…{RESET}gh")
        );
    }

    #[test]
    fn the_end_kept_starts_with_the_colors_in_effect_there() {
        assert_eq!(
            cut_path(&format!("{SUBDUED}abcdef"), columns(4)),
            format!("{SUBDUED}ab{RESET}{SUBDUED}…{RESET}{SUBDUED}f")
        );
        assert_eq!(
            cut_path(&format!("{STRONG}ab{RESET}cdefgh"), columns(4)),
            format!("{STRONG}ab{RESET}…{RESET}h")
        );
    }

    #[test]
    fn the_ellipsis_takes_the_colors_of_the_text_it_replaces() {
        assert_eq!(
            cut_path(&format!("ab{SUBDUED}cdefgh"), columns(4)),
            format!("ab{RESET}{SUBDUED}…{RESET}{SUBDUED}h")
        );
    }

    #[test]
    fn a_line_too_long_keeps_its_start_and_its_colors() {
        assert_eq!(cut("abcdef", columns(6)), "abcdef");
        assert_eq!(cut("abcdefgh", columns(6)), format!("abcde{RESET}…{RESET}"));
        assert_eq!(
            cut(&format!("{SUBDUED}abcdef"), columns(4)),
            format!("{SUBDUED}abc{RESET}{SUBDUED}…{RESET}")
        );
        assert_eq!(cut("ab日本", columns(4)), format!("ab{RESET}…{RESET}"));
    }

    #[test]
    fn a_wide_character_that_does_not_fit_is_cut_whole() {
        assert_eq!(cut_path("ab日本", columns(4)), format!("ab{RESET}…{RESET}"));
    }

    #[test]
    fn a_line_before_hidden_text_always_ends_in_an_ellipsis() {
        assert_eq!(
            cut_before_hidden("ab", columns(4)),
            format!("ab{RESET}…{RESET}")
        );
        assert_eq!(
            cut_before_hidden("abcd", columns(4)),
            format!("abc{RESET}…{RESET}")
        );
        assert_eq!(
            cut_before_hidden("abcdef", columns(4)),
            format!("abc{RESET}…{RESET}")
        );
    }

    #[test]
    fn a_spread_row_gives_up_its_right_first_then_its_left() {
        let right = [
            "a s │ q w 1/2".to_string(),
            "… 1/2".to_string(),
            "…".to_string(),
        ];
        let spread =
            |columns: usize| ScreenRow::spread("Code style", &right, self::columns(columns));
        assert_eq!(spread(30).as_str(), "Code style       a s │ q w 1/2");
        assert_eq!(spread(24).as_str(), "Code style a s │ q w 1/2");
        assert_eq!(spread(23).as_str(), "Code style        … 1/2");
        assert_eq!(spread(15).as_str(), "Code style    …");
        assert_eq!(spread(12).as_str(), "Code style …");
        assert_eq!(spread(11).as_str(), format!("Code sty{RESET}…{RESET} …"));
        assert_eq!(spread(2).as_str(), format!("C{RESET}…{RESET}"));
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
        let preview = Field {
            mark: Mark::Quote(Some("1".to_string())),
            ..field("x", 1)
        };
        let fitted = fit(&[field("question", 1), preview], 1, |_| 0);
        assert_eq!(shape(&fitted), [("… Ctrl+O shows the request", 1)]);
    }

    #[test]
    fn a_preview_gives_up_its_rows_down_to_one_as_any_value() {
        let preview = Field {
            mark: Mark::Quote(Some("1".to_string())),
            ..field("x\ny\nz", 5)
        };
        let fields = [field("question", 3), preview];
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
}
