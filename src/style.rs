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
pub const YELLOW: &str = "\x1b[33m";
pub const BLUE: &str = "\x1b[34m";
pub const MAGENTA: &str = "\x1b[35m";
pub const GREY: &str = "\x1b[90m";
/// Literal text, in backticks or a code block, as Markdown marks it.
pub const CODE: &str = "\x1b[36m";
/// The marks of the user's lines.
pub const LIGHT_CYAN: &str = "\x1b[96m";
/// For a warning.
pub const ORANGE: &str = "\x1b[38;5;208m";
/// The color of text that stays in the background.
pub const STATUS_COLOR: &str = "\x1b[38;5;244m";
/// The background of the user's lines, which sets them apart from Claude's.
pub const BAR: &str = "\x1b[48;5;236m";
