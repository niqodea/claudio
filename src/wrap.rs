//! The input box's text as the rows of the box: wrapped at the box's width, with the visual
//! selection in reverse video, and where the cursor is among the rows.

use unicode_width::UnicodeWidthChar;

// Swaps the colors of the selected text, and swaps them back after it
const REVERSE: &str = "\x1b[7m";
const UNREVERSE: &str = "\x1b[27m";

#[derive(Default)]
pub struct Row {
    /// The row's characters, with the codes that reverse the selection.
    pub text: String,
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
    let mut is_reversed = false;
    for (index, character) in text.chars().enumerate() {
        if character == '\n' {
            if index == cursor {
                cursor_at = Some((rows.len() - 1, column));
            }
            end_row(&mut rows, &mut is_reversed);
            column = 0;
            continue;
        }
        let shown = if character == '\t' { ' ' } else { character };
        let character_width = shown.width().unwrap_or(0);
        if column + character_width > width && column > 0 {
            end_row(&mut rows, &mut is_reversed);
            column = 0;
        }
        if index == cursor {
            cursor_at = Some((rows.len() - 1, column));
        }
        let is_selected = selection.is_some_and(|(start, end)| (start..end).contains(&index));
        let row = rows.last_mut().expect("rows start with one");
        if is_selected != is_reversed {
            row.text
                .push_str(if is_selected { REVERSE } else { UNREVERSE });
            is_reversed = is_selected;
        }
        row.text.push(shown);
        column += character_width;
        row.width = column;
    }
    let (cursor_row, cursor_column) = cursor_at.unwrap_or_else(|| {
        if column >= width {
            end_row(&mut rows, &mut is_reversed);
            column = 0;
        }
        (rows.len() - 1, column)
    });
    if is_reversed && let Some(row) = rows.last_mut() {
        row.text.push_str(UNREVERSE);
    }
    Wrapped {
        rows,
        cursor_row,
        cursor_column,
    }
}

/// Ends the last row, with its reversed text turned back, and starts a new one.
fn end_row(rows: &mut Vec<Row>, is_reversed: &mut bool) {
    if *is_reversed && let Some(row) = rows.last_mut() {
        row.text.push_str(UNREVERSE);
        *is_reversed = false;
    }
    rows.push(Row::default());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(wrapped: &Wrapped) -> Vec<&str> {
        wrapped.rows.iter().map(|row| row.text.as_str()).collect()
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
        assert_eq!(
            texts(&wrapped),
            [
                format!("a{REVERSE}bc{UNREVERSE}"),
                format!("{REVERSE}de{UNREVERSE}f")
            ]
        );
    }
}
