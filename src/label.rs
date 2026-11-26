//! The label rows, laid out in slots: parts whose columns and widths the screen's width alone
//! decides, so what one part shows never moves another, as the rows of the question view keep
//! their place by the screen's height alone.
//! A layout takes the screen's width and nothing of what the row shows, so no column can depend
//! on it, and each part's text then fills its slot, padded, or cut with `…`. A part with nothing
//! to show leaves its slot empty.
//!
//! On a narrow screen, each end of a label row keeps its edge and gives up text toward the
//! middle, so every `…` falls there: first the right end, the mode and the half-typed command
//! last, since they tell what the user is typing, then the label, which loses its end. What
//! either side gives up costs little, since the keys of a hidden name still work and the `…`
//! says that text is hidden, so the order stays this simple rather than weighing what each part
//! shows.
//! Ref: <https://ec.europa.eu/component-library/v3.13.0/ec/components/navigation/breadcrumb/code/>,
//! whose breadcrumbs keep their first and last items and hide the middle ones behind `…`
//! Ref: <https://github.com/nvim-lualine/lualine.nvim/wiki/Component-snippets>, which hides
//! whole components on a narrow window

use std::num::NonZeroUsize;

use unicode_width::UnicodeWidthChar;

use crate::cut::{ScreenRow, cut};
use crate::style::SUBDUED_ON_BAR;
use crate::text::{Line, Span};
use crate::wording::GROUP;

/// The columns kept for the keys of a half-typed vi command, which most commands fit, like `d2`
/// or `ci`. A longer one keeps its newest keys.
const PENDING_COLUMNS: usize = 3;

/// Where the keys go: the half-typed command's columns, a space, and the mode's letter.
const KEY_STATE_COLUMNS: usize = PENDING_COLUMNS + 2;

/// The names of a batch's questions, like `a s d f`, the most a batch holds.
const QUESTIONS_COLUMNS: usize = 7;

/// The names of the requests that wait, `q w e r`, the requests that have a key.
const REQUESTS_COLUMNS: usize = 7;

/// Where the keys go: the keys of a half-typed vi command, and the letter of the mode.
pub struct KeyState {
    pub pending: String,
    pub mode: char,
}

impl KeyState {
    /// The key state in its slot: the mode's letter at the slot's end, at the row's edge, and
    /// the half-typed command's keys in the columns before it, from their start, so a key once
    /// shown stays in its column as the next ones come.
    fn fill(&self) -> Line {
        let keys = newest(&self.pending, PENDING_COLUMNS);
        Line::from(Span::new(format!("{keys} {}", self.mode), SUBDUED_ON_BAR))
    }
}

/// The main screen's label row: the label, then where the keys go, at the row's end, which the
/// row keeps while the label has a column.
pub struct MainLabel {
    columns: NonZeroUsize,
    label: usize,
    has_key_state: bool,
}

impl MainLabel {
    pub fn new(columns: NonZeroUsize) -> Self {
        let right = 1 + KEY_STATE_COLUMNS;
        let has_key_state = columns.get() > right;
        let label = if has_key_state {
            columns.get() - right
        } else {
            columns.get()
        };
        Self {
            columns,
            label,
            has_key_state,
        }
    }

    pub fn row(&self, label: &Line, key_state: &KeyState) -> ScreenRow {
        let mut row = fill(label, self.label);
        if self.has_key_state {
            row.push(" ");
            row.append(key_state.fill());
        }
        ScreenRow::cut(&row, self.columns)
    }
}

/// What the question screen's label row shows at its right end, from its left: the names of the
/// batch's questions, none on a request of one question, those of the requests that wait, which
/// are always there, since the screen shows one, and where the keys go, none while the cursor is
/// on the options. The one part that may be missing is the outermost, so its empty slot merges
/// with the room after the label, and the row never shows a gap between two parts.
pub struct Progress {
    pub questions: Option<Line>,
    pub requests: Line,
    pub key_state: Option<KeyState>,
}

/// The parts of the question screen's label row's right end, from the one the row keeps longest
/// to the one it gives up first.
#[derive(Clone, Copy)]
enum Part {
    KeyState,
    Requests,
    Questions,
}

const PARTS: [Part; 3] = [Part::KeyState, Part::Requests, Part::Questions];

impl Part {
    /// The columns the part's slot takes, with the gap in front of it.
    fn columns(self) -> usize {
        match self {
            // The `│` in front of it, between two spaces
            Part::KeyState => 3 + KEY_STATE_COLUMNS,
            Part::Requests => 3 + REQUESTS_COLUMNS,
            Part::Questions => QUESTIONS_COLUMNS,
        }
    }
}

/// The question screen's label row: the label, then the progress at the row's end.
pub struct QuestionLabel {
    columns: NonZeroUsize,
    label: usize,
    /// How many parts the right end keeps, of `PARTS`, from its start.
    kept: usize,
}

impl QuestionLabel {
    /// The row on a screen `columns` wide, which keeps `label` columns for the label while it
    /// can. The right end gives up its parts first, whole, from its left: the questions, the
    /// requests, and where the keys go last, since it tells what the user is typing,
    /// and a `…` takes their place. Then the label loses its end. Each part keeps its slot
    /// whatever it shows, so a name never shows cut, and none moves when another changes.
    pub fn new(columns: NonZeroUsize, label: usize) -> Self {
        let whole = 1 + PARTS.iter().map(|part| part.columns()).sum::<usize>();
        if columns.get() >= label + whole {
            return Self {
                columns,
                label: columns.get() - whole,
                kept: PARTS.len(),
            };
        }
        // The gap after the label, and the `…` of what the row gives up
        let mut right = 2;
        let mut kept = 0;
        for part in PARTS {
            if columns.get() < label + right + part.columns() {
                break;
            }
            right += part.columns();
            kept += 1;
        }
        // A `…` alone still says that the right end hides its parts, beside a column of label
        if kept == 0 && columns.get() <= right {
            right = 0;
        }
        Self {
            columns,
            label: columns.get() - right,
            kept,
        }
    }

    fn keeps(&self, part: Part) -> bool {
        (part as usize) < self.kept
    }

    pub fn row(&self, label: &Line, progress: &Progress) -> ScreenRow {
        let mut row = fill(label, self.label);
        if self.label == self.columns.get() {
            return ScreenRow::cut(&row, self.columns);
        }
        row.push(" ");
        if self.kept < PARTS.len() {
            let hides = (!self.keeps(Part::Questions) && progress.questions.is_some())
                || !self.keeps(Part::Requests)
                || (!self.keeps(Part::KeyState) && progress.key_state.is_some());
            let sign = if hides { "…" } else { "" };
            row.append(fill(&Line::from(Span::new(sign, SUBDUED_ON_BAR)), 1));
        }
        if self.keeps(Part::Questions) {
            let questions = progress.questions.clone().unwrap_or_default();
            row.append(fill(&questions, QUESTIONS_COLUMNS));
        }
        if self.keeps(Part::Requests) {
            // The bar sets the requests apart from the questions, when both show
            let gap = if self.keeps(Part::Questions) && progress.questions.is_some() {
                GROUP
            } else {
                "   "
            };
            let mut requests = Line::from(Span::new(gap, SUBDUED_ON_BAR));
            requests.append(progress.requests.clone());
            row.append(fill(&requests, Part::Requests.columns()));
        }
        // A `│` sets where the keys go apart, as between the other groups
        if self.keeps(Part::KeyState) {
            let mut part = Line::from(Span::new(GROUP, SUBDUED_ON_BAR));
            part.append((progress.key_state.as_ref()).map_or_else(Line::default, KeyState::fill));
            row.append(fill(&part, Part::KeyState.columns()));
        }
        ScreenRow::cut(&row, self.columns)
    }
}

/// `text` in a slot `width` columns wide: padded at its end, or cut there with `…`.
fn fill(text: &Line, width: usize) -> Line {
    let Some(columns) = NonZeroUsize::new(width) else {
        return Line::default();
    };
    let text = cut(text, columns);
    let pad = width.saturating_sub(text.width());
    text.with(" ".repeat(pad))
}

/// The newest keys of `keys`, a half-typed command, that fit in `width` columns, padded at their
/// end: as they grow past it, each new key pushes out the oldest, with no `…`, as vim's
/// `showcmd` does, so the key just typed always shows, and the others keep their order. The keys
/// are the user's own, just typed, so they need no sign of the cut.
/// Ref: `add_to_showcmd` in vim's src/normal.c, which drops the oldest keys past its columns
fn newest(keys: &str, width: usize) -> String {
    let mut used = 0;
    let mut kept = Vec::new();
    for key in keys.chars().rev() {
        let key_width = key.width().unwrap_or(0);
        if used + key_width > width {
            break;
        }
        used += key_width;
        kept.push(key);
    }
    let kept: String = kept.into_iter().rev().collect();
    format!("{kept}{}", " ".repeat(width - used))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn columns(count: usize) -> NonZeroUsize {
        NonZeroUsize::new(count).expect("a column")
    }

    /// `row` without its styles.
    fn plain(row: &ScreenRow) -> String {
        row.line().text()
    }

    fn key_state(pending: &str, mode: char) -> KeyState {
        KeyState {
            pending: pending.to_string(),
            mode,
        }
    }

    fn progress(questions: &str, requests: &str, key: Option<KeyState>) -> Progress {
        Progress {
            questions: (!questions.is_empty()).then(|| Line::from(questions)),
            requests: Line::from(requests),
            key_state: key,
        }
    }

    #[test]
    fn a_half_typed_command_moves_nothing_on_the_main_label_row() {
        let layout = MainLabel::new(columns(30));
        let idle = plain(&layout.row(&Line::from("›› idle"), &key_state("", 'N')));
        let typing = plain(&layout.row(&Line::from("›› idle"), &key_state("d2", 'N')));
        assert_eq!(idle, "›› idle                      N");
        assert_eq!(typing, "›› idle                  d2  N");
    }

    #[test]
    fn a_half_typed_command_moves_nothing_on_the_question_label_row() {
        let layout = QuestionLabel::new(columns(60), 20);
        let row = |pending: &str| {
            plain(&layout.row(
                &Line::from("›› Code style"),
                &progress("a s d f", "q w · ·", Some(key_state(pending, 'N'))),
            ))
        };
        assert_eq!(
            row(""),
            "›› Code style                      a s d f │ q w · · │     N"
        );
        assert_eq!(
            row("d"),
            "›› Code style                      a s d f │ q w · · │ d   N"
        );
        assert_eq!(
            row("d2"),
            "›› Code style                      a s d f │ q w · · │ d2  N"
        );
        assert_eq!(
            row("100d"),
            "›› Code style                      a s d f │ q w · · │ 00d N"
        );
    }

    #[test]
    fn each_part_keeps_its_columns_from_one_request_to_the_next() {
        let layout = QuestionLabel::new(columns(60), 20);
        let two = plain(&layout.row(
            &Line::from("›› Code style"),
            &progress("a s · ·", "q w e ·", None),
        ));
        let one = plain(&layout.row(&Line::from("›› Code style"), &progress("", "q w e ·", None)));
        let alone = plain(&layout.row(
            &Line::from("›› Code style"),
            &progress("a s · ·", "q · · ·", None),
        ));
        assert_eq!(
            alone,
            "›› Code style                      a s · · │ q · · · │      "
        );
        assert_eq!(
            two,
            "›› Code style                      a s · · │ q w e · │      "
        );
        // The questions, missing, leave room at the outer edge, and no gap between parts
        assert_eq!(
            one,
            "›› Code style                                q w e · │      "
        );
    }

    #[test]
    fn a_narrow_screen_gives_up_the_parts_from_the_left_whatever_the_label_says() {
        let full = progress("a s · ·", "q w · ·", Some(key_state("", 'I')));
        let row = |label: &str, count: usize| {
            plain(&QuestionLabel::new(columns(count), 20).row(&Line::from(label), &full))
        };
        let short = "›› Code style";
        assert_eq!(
            row(short, 46),
            "›› Code style        a s · · │ q w · · │     I"
        );
        assert_eq!(
            row(short, 45),
            "›› Code style             …   q w · · │     I"
        );
        assert_eq!(row(short, 39), "›› Code style                 … │     I");
        assert_eq!(row(short, 22), "›› Code style        …");
        // A longer label loses its end, and the right end stays as it was
        let long = "›› Code style │ a long hint that does not fit";
        for count in [46, 45, 39, 22] {
            let width = QuestionLabel::new(columns(count), 20).label;
            let right = |label| row(label, count).chars().skip(width).collect::<String>();
            assert_eq!(right(short), right(long), "{count} columns");
        }
    }

    #[test]
    fn a_part_given_up_with_nothing_to_show_leaves_no_sign() {
        let layout = QuestionLabel::new(columns(44), 20);
        let row = plain(&layout.row(&Line::from("›› Code style"), &progress("", "q · · ·", None)));
        assert!(!row.contains('…'), "{row}");
    }

    #[test]
    fn every_row_takes_the_whole_screen() {
        let full = progress("a s d f", "q w e r", Some(key_state("100d", 'N')));
        for count in 1..80 {
            let question = QuestionLabel::new(columns(count), 20)
                .row(&Line::from("›› Code style │ hint"), &full);
            let main = MainLabel::new(columns(count))
                .row(&Line::from("›› T1 S1 W2 │ idle"), &key_state("d", 'N'));
            assert_eq!(question.line().width(), count, "{}", plain(&question));
            assert_eq!(main.line().width(), count, "{}", plain(&main));
        }
    }
}
