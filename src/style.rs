//! claudio's color scheme: the styles text takes, each named by what it means, as the README's
//! Colors table lists them. Code outside this module styles text by these meanings alone, since
//! the raw codes are private to it, so a style keeps its one meaning however the code grows, and
//! a new meaning starts as a row of the table and a name here. Color never carries meaning
//! alone: whatever a color says, a character on the same line says too, so the scrollback reads
//! the same without color, as once copied.
//!
//! A style is a value, whole in itself, so where it is set says nothing of the styles around
//! it: `text.rs` gives each piece of text its style, and writes the codes only as the text
//! reaches the terminal.
//! Ref: <https://en.wikipedia.org/wiki/ANSI_escape_code#Select_Graphic_Rendition_parameters>

use std::fmt::Write;

/// The look of a piece of text: a color in front and behind, each the terminal's own when none,
/// and the attributes on it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    foreground: Option<Color>,
    background: Option<Color>,
    is_bold: bool,
    is_dim: bool,
    is_reversed: bool,
}

/// A color, as the parameter that sets it in front of the text, which `+ 10` sets behind it for
/// the 16 colors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Color {
    /// One of the terminal's 16 colors, which its theme sets, by its foreground parameter.
    Theme(u8),
    /// One of the 256 colors, a fixed shade.
    Fixed(u8),
}

impl Style {
    /// `self` on `under`: each color of `self`'s, and `under`'s where `self` has none, as text
    /// drawn on a background keeps its own colors and takes the background's where it has none,
    /// and the attributes of both, like bold, which add up.
    pub fn on(self, under: Style) -> Style {
        Style {
            foreground: self.foreground.or(under.foreground),
            background: self.background.or(under.background),
            is_bold: self.is_bold || under.is_bold,
            is_dim: self.is_dim || under.is_dim,
            is_reversed: self.is_reversed || under.is_reversed,
        }
    }

    /// `self` with its colors swapped, as of the selected text.
    pub fn reversed(self) -> Style {
        Style {
            is_reversed: true,
            ..self
        }
    }

    /// The code that sets this style from any other: a reset, then what the style sets.
    pub fn code(self) -> String {
        let mut code = "\x1b[0".to_string();
        let attributes = [(self.is_bold, 1), (self.is_dim, 2), (self.is_reversed, 7)];
        for (_, parameter) in attributes.iter().filter(|(is_set, _)| *is_set) {
            let _ = write!(code, ";{parameter}");
        }
        for (color, layer) in [(self.foreground, 0), (self.background, 10)] {
            match color {
                Some(Color::Theme(parameter)) => {
                    let _ = write!(code, ";{}", parameter + layer);
                }
                Some(Color::Fixed(index)) => {
                    let _ = write!(code, ";{};5;{index}", 38 + layer);
                }
                None => {}
            }
        }
        code.push('m');
        code
    }
}

// The raw styles, which only the meanings below use
const BOLD: Style = Style {
    is_bold: true,
    ..PLAIN
};
const DIM: Style = Style {
    is_dim: true,
    ..PLAIN
};
const REVERSED: Style = Style {
    is_reversed: true,
    ..PLAIN
};
const RED: Style = foreground(Color::Theme(31));
const GREEN: Style = foreground(Color::Theme(32));
const MAGENTA: Style = foreground(Color::Theme(35));
const CYAN: Style = foreground(Color::Theme(36));
const ORANGE: Style = foreground(Color::Fixed(208));
/// Light enough for a contrast of 4.5:1 on the bar, where dim text would not read.
const BAR_GREY: Style = foreground(Color::Fixed(248));
const BAR_BACKGROUND: Style = Style {
    background: Some(Color::Fixed(236)),
    ..PLAIN
};

const fn foreground(color: Color) -> Style {
    Style {
        foreground: Some(color),
        ..PLAIN
    }
}

/// The terminal's own colors, for what the user and Claude write.
pub const PLAIN: Style = Style {
    foreground: None,
    background: None,
    is_bold: false,
    is_dim: false,
    is_reversed: false,
};
/// Headings, strong text, tool names, question names and the option under the cursor: what to
/// read first.
pub const STRONG: Style = BOLD;
/// Literal text, in backticks or a code block without a language, as Markdown marks it: in
/// bold, which has no hue, so it reads on any theme, and the backticks tell it from strong text.
/// A background would read as the user's, as the bar does.
pub const CODE: Style = BOLD;
/// What stays in the background, like a call's result, the marks that structure Markdown, or
/// claudio's own notes, everywhere but on the bar.
pub const SUBDUED: Style = DIM;
/// What stays in the background, on the bar, where `SUBDUED` would not read.
pub const SUBDUED_ON_BAR: Style = BAR_GREY;
/// The background of what is the user's: their lines and the input box.
pub const YOURS: Style = BAR_BACKGROUND;
/// The marks of the conversation: those of the user's lines and of Claude's replies, the
/// cursor among options, and the command line's prompt. Cyan, which reads clearly on a dark
/// background in most themes, where magenta faded into it.
pub const MARK: Style = CYAN;
/// What needs the user's attention, like a warning, a key that waits for a second press, or a
/// cache that turns cold.
pub const ATTENTION: Style = ORANGE;
/// What waits for the user's answer: the requests Claude makes. Reversed, as tmux marks a window
/// that alerts, so it stands out on any theme in no hue of its own.
pub const AWAITS_YOU: Style = REVERSED;
/// What failed or was removed: an error, a limit reached, the lines a diff removes. Red reads as
/// an error, so it says nothing else.
pub const FAILED_OR_REMOVED: Style = RED;
/// What is done or added: a submitted answer, the lines a diff adds.
pub const DONE_OR_ADDED: Style = GREEN;

/// The colors of code in a language, which its grammar picks, and the terminal's palette sets.
/// They mean what the grammar says, so they are apart from the meanings above.
pub mod grammar {
    use super::{Color, Style, foreground};

    pub const RED: Style = super::RED;
    pub const GREEN: Style = super::GREEN;
    pub const YELLOW: Style = foreground(Color::Theme(33));
    pub const BLUE: Style = foreground(Color::Theme(34));
    pub const MAGENTA: Style = super::MAGENTA;
    pub const GREY: Style = foreground(Color::Theme(90));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_resets_then_sets_the_whole_style() {
        assert_eq!(PLAIN.code(), "\x1b[0m");
        assert_eq!(STRONG.on(FAILED_OR_REMOVED).code(), "\x1b[0;1;31m");
        assert_eq!(
            SUBDUED_ON_BAR.on(YOURS).reversed().code(),
            "\x1b[0;7;38;5;248;48;5;236m"
        );
    }

    #[test]
    fn a_style_keeps_its_own_colors_on_another() {
        assert_eq!(MARK.on(ATTENTION), MARK);
        assert_eq!(PLAIN.on(YOURS), YOURS);
    }
}
