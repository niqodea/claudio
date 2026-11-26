//! A Markdown table, laid out once all its rows arrived: as a grid, with each column as wide as
//! its widest cell, or, when the grid is wider than a fixed number of columns, as one numbered
//! record per row, each field on a line of its own, as psql's expanded display shows a row. Either
//! layout is still Markdown, so it copies back as the same table, or as a list of the same cells.
//! Ref: <https://www.postgresql.org/docs/current/app-psql.html>, on `\x auto`, which shows a
//! table too wide for the screen one record at a time

use unicode_width::UnicodeWidthStr;

use crate::style::{STRONG, SUBDUED};
use crate::text::{Line, Span};

// The narrowest divider cell Markdown reads as one
const MIN_COLUMN_WIDTH: usize = 3;

pub struct Table {
    header: Vec<String>,
    alignments: Vec<Alignment>,
    rows: Vec<Vec<String>>,
    widths: Vec<usize>,
}

#[derive(Clone, Copy)]
enum Alignment {
    Left,
    Center,
    Right,
}

impl Table {
    /// Reads `lines` as a header row, a divider row and body rows that each have as many cells as
    /// the header. None for anything else, which is then shown as it was written.
    pub fn read(lines: &[String]) -> Option<Table> {
        let [header, divider, body @ ..] = lines else {
            return None;
        };
        let header = split_row(header)?;
        let alignments = split_row(divider)?
            .iter()
            .map(|cell| read_alignment(cell))
            .collect::<Option<Vec<_>>>()?;
        let rows = body
            .iter()
            .map(|line| split_row(line))
            .collect::<Option<Vec<_>>>()?;
        if alignments.len() != header.len() || rows.iter().any(|row| row.len() != header.len()) {
            return None;
        }
        let widths = (0..header.len())
            .map(|column| {
                std::iter::once(&header)
                    .chain(&rows)
                    .map(|row| row[column].width())
                    .fold(MIN_COLUMN_WIDTH, usize::max)
            })
            .collect();
        Some(Table {
            header,
            alignments,
            rows,
            widths,
        })
    }

    /// The lines of the laid-out table, as a grid when it takes at most `max_columns`, and as
    /// records otherwise, with the body's cells as `color_cell` colors them.
    pub fn render(&self, color_cell: impl Fn(&str) -> Line, max_columns: usize) -> Vec<Line> {
        if self.grid_width() <= max_columns {
            self.render_grid(color_cell)
        } else {
            self.render_records(color_cell)
        }
    }

    /// The columns the grid takes: each column's cell between a space on either side and a pipe
    /// after it, and the pipe that starts the row.
    fn grid_width(&self) -> usize {
        1 + self.widths.iter().map(|width| width + 3).sum::<usize>()
    }

    /// The grid, header in bold.
    fn render_grid(&self, color_cell: impl Fn(&str) -> Line) -> Vec<Line> {
        let pipe = || Span::new("|", SUBDUED);
        let mut header = Line::from(pipe());
        for (column, cell) in self.header.iter().enumerate() {
            header.append(self.pad(column, cell, Line::from(Span::new(cell, STRONG))));
            header.push(pipe());
        }
        let mut divider = String::from("|");
        for (alignment, width) in self.alignments.iter().zip(&self.widths) {
            let dashes = match alignment {
                Alignment::Left => "-".repeat(*width),
                Alignment::Center => format!(":{}:", "-".repeat(width - 2)),
                Alignment::Right => format!("{}:", "-".repeat(width - 1)),
            };
            divider.push(' ');
            divider.push_str(&dashes);
            divider.push_str(" |");
        }
        let mut lines = vec![header, Line::from(Span::new(divider, SUBDUED))];
        for row in &self.rows {
            let mut line = Line::from(pipe());
            for (column, cell) in row.iter().enumerate() {
                line.append(self.pad(column, cell, color_cell(cell)));
                line.push(pipe());
            }
            lines.push(line);
        }
        lines
    }

    /// One record per row, as an ordered list: the row's number, then each field as a list item
    /// under it, the header in bold, then the cell, or `(empty)` in grey for an empty one. Every
    /// field takes the same shape, the first one too, since the first column names the row only
    /// most of the time.
    ///
    /// A value follows its header after `: `, so a long header, like `Creator's share of the
    /// average`, pushes only its own value to the right. psql and Codex align a record's values
    /// in one column, which scans faster, but pads every field to the longest header, and Codex
    /// then needs a rule to drop the alignment when the headers are too wide.
    /// Ref: <https://github.com/openai/codex/blob/rust-v0.171.0/codex-rs/tui/src/markdown_render/table_key_value.rs>
    ///
    /// A record's number stands alone on its line, which CommonMark reads as an item that starts
    /// with a blank line, and an item like that cannot interrupt a paragraph. So this assumes
    /// Claude starts a table after a blank line, as Claude does in practice: once copied, a table
    /// right under a line of text would lose its first number into that line.
    /// Ref: <https://spec.commonmark.org/0.31.2/#list-items>
    fn render_records(&self, color_cell: impl Fn(&str) -> Line) -> Vec<Line> {
        // CommonMark takes the lines under a marker of width W, like `10.`, into its item when they
        // are W + 1 columns deep, and allows up to 3 columns more. Every field goes as deep as the
        // widest number needs, so the narrower numbers' fields go deeper than they need, which
        // holds below 10,000 rows. Past that, `1.`'s fields go 4 columns deeper than it needs,
        // and once copied, they read as a code block.
        let depth = format!("{}.", self.rows.len()).len() + 1;
        let mut lines = Vec::new();
        for (index, row) in self.rows.iter().enumerate() {
            lines.push(Line::from(Span::new(format!("{}.", index + 1), SUBDUED)));
            for (header, cell) in self.header.iter().zip(row) {
                let mut field = Line::from(" ".repeat(depth)).with(Span::new("-", SUBDUED));
                field.push(" ");
                if !header.is_empty() {
                    field.push(Span::new(header, STRONG));
                    field.push(": ");
                }
                if cell.is_empty() {
                    field.push(Span::new("(empty)", SUBDUED));
                } else {
                    field.append(color_cell(cell));
                }
                lines.push(field);
            }
        }
        lines
    }

    /// The cell's `text`, colored, with the spaces that align `cell` in its column around it.
    fn pad(&self, column: usize, cell: &str, text: Line) -> Line {
        let space = self.widths[column] - cell.width();
        let (before, after) = match self.alignments[column] {
            Alignment::Left => (0, space),
            Alignment::Center => (space / 2, space - space / 2),
            Alignment::Right => (space, 0),
        };
        let mut padded = Line::from(" ".repeat(before + 1));
        padded.append(text);
        padded.with(" ".repeat(after + 1))
    }
}

/// The cells of a row, which starts with a pipe and may end with one. A pipe escaped with a
/// backslash, or inside a code span, is text rather than the end of a cell.
fn split_row(line: &str) -> Option<Vec<String>> {
    let inside = line.trim().strip_prefix('|')?;
    let mut cells = vec![String::new()];
    let mut is_code_span = false;
    let mut characters = inside.chars().peekable();
    while let Some(character) = characters.next() {
        let cell = cells.last_mut().expect("the row starts with a cell");
        match character {
            '\\' if characters.peek() == Some(&'|') => {
                cell.push('\\');
                cell.push(characters.next().expect("the pipe was peeked"));
            }
            '`' => {
                is_code_span = !is_code_span;
                cell.push('`');
            }
            '|' if !is_code_span => cells.push(String::new()),
            character => cell.push(character),
        }
    }
    // The pipe that ends a row leaves nothing after it
    if cells.last().is_some_and(String::is_empty) {
        cells.pop();
    }
    Some(cells.iter().map(|cell| cell.trim().to_string()).collect())
}

/// The alignment a divider cell like `---`, `:--:` or `--:` sets for its column.
fn read_alignment(cell: &str) -> Option<Alignment> {
    let dashes = cell.trim_start_matches(':').trim_end_matches(':');
    if dashes.is_empty() || !dashes.chars().all(|character| character == '-') {
        return None;
    }
    match (cell.starts_with(':'), cell.ends_with(':')) {
        (true, true) => Some(Alignment::Center),
        (false, true) => Some(Alignment::Right),
        (_, false) => Some(Alignment::Left),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(lines: &[&str]) -> Option<Table> {
        let lines: Vec<String> = lines.iter().map(ToString::to_string).collect();
        Table::read(&lines)
    }

    fn render_plain(table: &Table) -> String {
        render_plain_within(table, usize::MAX)
    }

    fn render_plain_within(table: &Table, max_columns: usize) -> String {
        let lines = table.render(|cell| Line::from(cell), max_columns);
        lines.iter().map(Line::text).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn a_table_as_wide_as_the_limit_stays_a_grid() {
        let table = read(&["| a | b |", "|---|---|", "| x | y |"]).expect("a table");
        assert_eq!(table.grid_width(), 13);
        assert!(render_plain_within(&table, 13).starts_with("| a "));
        assert!(render_plain_within(&table, 12).starts_with("1."));
    }

    #[test]
    fn a_record_gives_every_cell_a_line_under_the_row_s_number() {
        let table = read(&[
            "| Fruit | Color |",
            "|---|---|",
            "| Apple | red |",
            "| Banana | |",
        ])
        .expect("a table");
        assert_eq!(
            render_plain_within(&table, 1),
            "1.\n   - Fruit: Apple\n   - Color: red\n2.\n   - Fruit: Banana\n   - Color: (empty)"
        );
    }

    #[test]
    fn a_field_of_a_column_without_a_header_is_its_cell_alone() {
        let table = read(&["| | A |", "|---|---|", "| Size | 3 |"]).expect("a table");
        assert_eq!(render_plain_within(&table, 1), "1.\n   - Size\n   - A: 3");
    }

    #[test]
    fn every_field_goes_as_deep_as_the_widest_number_needs() {
        let mut lines = vec!["| a |", "|---|"];
        lines.extend(["| x |"; 10]);
        let rendered = render_plain_within(&read(&lines).expect("a table"), 1);
        assert!(rendered.starts_with("1.\n    - a: x\n2."));
        assert!(rendered.ends_with("10.\n    - a: x"));
    }

    #[test]
    fn columns_take_the_width_of_their_widest_cell() {
        let table = read(&[
            "| Name | Age |",
            "|---|---|",
            "| Ada | 36 |",
            "| Grace | 85 |",
        ])
        .expect("a table");
        assert_eq!(
            render_plain(&table),
            "| Name  | Age |\n| ----- | --- |\n| Ada   | 36  |\n| Grace | 85  |"
        );
    }

    #[test]
    fn a_divider_aligns_its_column() {
        let table = read(&["| a | b |", "|:-:|--:|", "| x | 10000 |"]).expect("a table");
        assert_eq!(
            render_plain(&table),
            "|  a  |     b |\n| :-: | ----: |\n|  x  | 10000 |"
        );
    }

    #[test]
    fn a_pipe_in_code_or_escaped_stays_in_its_cell() {
        let table = read(&["| a | b |", "|---|---|", "| `x | y` | 1 \\| 2 |"]).expect("a table");
        assert_eq!(table.rows, vec![vec!["`x | y`", "1 \\| 2"]]);
    }

    #[test]
    fn a_wide_character_counts_as_two_columns() {
        let table = read(&["| a |", "|---|", "| 日本 |"]).expect("a table");
        assert_eq!(render_plain(&table), "| a    |\n| ---- |\n| 日本 |");
    }

    #[test]
    fn a_row_with_another_number_of_cells_is_no_table() {
        assert!(read(&["| a | b |", "|---|---|", "| x |"]).is_none());
    }

    #[test]
    fn rows_without_a_divider_are_no_table() {
        assert!(read(&["| a | b |", "| x | y |"]).is_none());
    }

    #[test]
    fn a_lone_row_is_no_table() {
        assert!(read(&["| a | b |"]).is_none());
    }
}
