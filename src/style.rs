//! The codes that style text on the terminal, each defined once, with what it says where it
//! says one thing. Color never carries meaning alone: whatever a color says, a character on the
//! same line says too, so the scrollback reads the same without color, as once copied.
//! Ref: <https://en.wikipedia.org/wiki/ANSI_escape_code#Select_Graphic_Rendition_parameters>

pub const RESET: &str = "\x1b[0m";
pub const BOLD: &str = "\x1b[1m";
/// For what supports the text, like what Claude did, and the marks that structure Markdown.
pub const DIM: &str = "\x1b[2m";
/// Swaps the colors, as of the selected text, and swaps them back after it.
pub const REVERSE: &str = "\x1b[7m";
pub const UNREVERSE: &str = "\x1b[27m";

/// For failures alone, since red reads as an error, and what a diff removes.
pub const RED: &str = "\x1b[31m";
/// For what was added, and what the user submitted.
pub const GREEN: &str = "\x1b[32m";
// Code's colors, which its grammar picks, and magenta for the marks too
pub const YELLOW: &str = "\x1b[33m";
pub const BLUE: &str = "\x1b[34m";
pub const MAGENTA: &str = "\x1b[35m";
pub const GREY: &str = "\x1b[90m";
/// Literal text, in backticks or a code block without a language, as Markdown marks it: in
/// bold, which has no hue, so it reads on any theme, and the backticks tell it from strong text.
/// A background would read as the user's, as the bar does.
pub const CODE: &str = BOLD;
/// The marks of the conversation: those of the user's lines and of Claude's replies, and the
/// cursor among options. Warm by choice, where cyan read as cold.
pub const MARK: &str = MAGENTA;
/// For what needs the user's attention, like a warning or a cache that turns cold.
pub const ORANGE: &str = "\x1b[38;5;208m";
/// The color of text that stays in the background, light enough for a contrast of 4.5:1 on the
/// bar.
pub const STATUS_COLOR: &str = "\x1b[38;5;248m";
/// The background of the user's lines, which sets them apart from Claude's.
pub const BAR: &str = "\x1b[48;5;236m";
