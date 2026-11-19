//! The input box's text as the rows of the box: wrapped at the box's width, with the visual
//! selection in reverse video, and where the cursor is among the rows.

use unicode_width::UnicodeWidthChar;

use crate::style::PLAIN;
use crate::text::{Line, Span};

#[derive(Default)]
pub struct Row {
    /// The row's characters, the selection reversed.
    pub text: Line,
    /// The columns the row's characters take.
    pub width: usize,
}

pub struct Wrapped {
    pub rows: Vec<Row>,
    pub cursor_row: usize,
    pub cursor_column: usize,
}

/// Wraps `text` at `width` columns, with the characters from `selection.0` to before
/// `selection.1` reversed. A character too wide for the rest of a row starts the next row. The
/// cursor at the end of a full row starts a row of its own, so it stays in the box. A tab shows
/// as one space.
pub fn wrap(text: &str, cursor: usize, selection: Option<(usize, usize)>, width: usize) -> Wrapped {
    let width = width.max(1);
    let mut rows = vec![Row::default()];
    let mut column = 0;
    let mut cursor_at = None;
    for (index, character) in text.chars().enumerate() {
        if character == '\n' {
            if index == cursor {
                cursor_at = Some((rows.len() - 1, column));
            }
            rows.push(Row::default());
            column = 0;
            continue;
        }
        let shown = if character == '\t' { ' ' } else { character };
        let character_width = shown.width().unwrap_or(0);
        if column + character_width > width && column > 0 {
            rows.push(Row::default());
            column = 0;
        }
        if index == cursor {
            cursor_at = Some((rows.len() - 1, column));
        }
        let is_selected = selection.is_some_and(|(start, end)| (start..end).contains(&index));
        let row = rows.last_mut().expect("rows start with one");
        let style = if is_selected { PLAIN.reversed() } else { PLAIN };
        row.text.push(Span::new(shown, style));
        column += character_width;
        row.width = column;
    }
    let (cursor_row, cursor_column) = cursor_at.unwrap_or_else(|| {
        if column >= width {
            rows.push(Row::default());
            column = 0;
        }
        (rows.len() - 1, column)
    });
    Wrapped {
        rows,
        cursor_row,
        cursor_column,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(wrapped: &Wrapped) -> Vec<String> {
        wrapped.rows.iter().map(|row| row.text.text()).collect()
    }

    #[test]
    fn a_line_wider_than_the_box_wraps_at_its_width() {
        let wrapped = wrap("abcdefg", 3, None, 3);
        assert_eq!(texts(&wrapped), ["abc", "def", "g"]);
        assert_eq!((wrapped.cursor_row, wrapped.cursor_column), (1, 0));
    }

    #[test]
    fn line_breaks_start_rows() {
        let wrapped = wrap("ab\n\ncd", 3, None, 10);
        assert_eq!(texts(&wrapped), ["ab", "", "cd"]);
        assert_eq!((wrapped.cursor_row, wrapped.cursor_column), (1, 0));
    }

    #[test]
    fn the_cursor_after_a_full_row_starts_a_row_of_its_own() {
        let wrapped = wrap("abc", 3, None, 3);
        assert_eq!(texts(&wrapped), ["abc", ""]);
        assert_eq!((wrapped.cursor_row, wrapped.cursor_column), (1, 0));
    }

    #[test]
    fn a_wide_character_that_does_not_fit_starts_the_next_row() {
        let wrapped = wrap("ab日", 0, None, 3);
        assert_eq!(texts(&wrapped), ["ab", "日"]);
        assert_eq!(wrapped.rows[1].width, 2);
    }

    #[test]
    fn the_selection_is_reversed_on_every_row_it_spans() {
        let wrapped = wrap("abcdef", 0, Some((1, 5)), 3);
        let reversed = |text| Span::new(text, PLAIN.reversed());
        let rows: Vec<&Line> = wrapped.rows.iter().map(|row| &row.text).collect();
        assert_eq!(
            rows,
            [
                &Line::from(["a".into(), reversed("bc")]),
                &Line::from([reversed("de"), "f".into()])
            ]
        );
    }
}
