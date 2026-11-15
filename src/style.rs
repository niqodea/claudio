//! claudio's color scheme: the styles text takes, each named by what it means, as the README's
//! Colors table lists them. Code outside this module styles text by these meanings alone, since
//! the raw codes are private to it, so a style keeps its one meaning however the code grows, and
//! a new meaning starts as a row of the table and a name here. Color never carries meaning
//! alone: whatever a color says, a character on the same line says too, so the scrollback reads
//! the same without color, as once copied.
//! Ref: <https://en.wikipedia.org/wiki/ANSI_escape_code#Select_Graphic_Rendition_parameters>

// The raw codes, which only the meanings below use
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const MAGENTA: &str = "\x1b[35m";
const ORANGE: &str = "\x1b[38;5;208m";
/// Light enough for a contrast of 4.5:1 on the bar, where dim text would not read.
const BAR_GREY: &str = "\x1b[38;5;248m";
const BAR_BACKGROUND: &str = "\x1b[48;5;236m";

/// The default color, for what the user and Claude write.
pub const RESET: &str = "\x1b[0m";
/// Headings, strong text, tool names, question names and the option under the cursor: what to
/// read first.
pub const STRONG: &str = BOLD;
/// Literal text, in backticks or a code block without a language, as Markdown marks it: in
/// bold, which has no hue, so it reads on any theme, and the backticks tell it from strong text.
/// A background would read as the user's, as the bar does.
pub const CODE: &str = BOLD;
/// What stays in the background, like a call's result, the marks that structure Markdown, or
/// claudio's own notes, everywhere but on the bar.
pub const SUBDUED: &str = DIM;
/// What stays in the background, on the bar, where `SUBDUED` would not read.
pub const SUBDUED_ON_BAR: &str = BAR_GREY;
/// The background of what is the user's: their lines and the input box.
pub const YOURS: &str = BAR_BACKGROUND;
/// The marks of the conversation: those of the user's lines and of Claude's replies, and the
/// cursor among options. Warm by choice, where cyan read as cold.
pub const MARK: &str = MAGENTA;
/// What needs the user's attention, like a warning, a key that waits for a second press, or a
/// cache that turns cold.
pub const ATTENTION: &str = ORANGE;
/// What failed or was removed: an error, a limit reached, the lines a diff removes. Red reads as
/// an error, so it says nothing else.
pub const FAILED_OR_REMOVED: &str = RED;
/// What is done or added: a submitted answer, the lines a diff adds.
pub const DONE_OR_ADDED: &str = GREEN;
/// Swaps the colors, as of the selected text, and swaps them back after it.
pub const REVERSE: &str = "\x1b[7m";
pub const UNREVERSE: &str = "\x1b[27m";

/// The colors of code in a language, which its grammar picks, and the terminal's palette sets.
/// They mean what the grammar says, so they are apart from the meanings above.
pub mod grammar {
    pub const RED: &str = super::RED;
    pub const GREEN: &str = super::GREEN;
    pub const YELLOW: &str = "\x1b[33m";
    pub const BLUE: &str = "\x1b[34m";
    pub const MAGENTA: &str = super::MAGENTA;
    pub const GREY: &str = "\x1b[90m";
}
