//! The user's settings, from `claudio/config.toml` in `$XDG_CONFIG_HOME`, or in `~/.config` when
//! that is unset or empty. Each setting the file leaves out keeps its default, and so does each
//! one when there is no file.

use std::env;
use std::fs;
use std::io::ErrorKind;
use std::num::{NonZeroU16, NonZeroUsize};
use std::path::PathBuf;

use serde::Deserialize;

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// The rows of the input box.
    pub input_rows: NonZeroU16,
    /// The columns a noisy line takes at most on screen: a line that shows a tool's input or
    /// output as it is.
    pub max_noisy_line_columns: NonZeroUsize,
    /// The columns a table of Claude's takes at most as a grid, past which it shows one record
    /// per row.
    pub max_table_columns: NonZeroUsize,
    /// How Claude's text shows while Claude writes it.
    pub stream: Stream,
    /// Where the terminal breaks Claude's prose.
    pub prose_wrap: ProseWrap,
}

/// Where the terminal breaks Claude's prose, as the README's Wrapping at spaces tells.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ProseWrap {
    /// At the screen's edge, as every terminal does.
    Edge,
    /// At spaces, once claudio asked the terminal and it said it can.
    AskSpaces,
}

/// How Claude's text shows while Claude writes it.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Stream {
    /// Each piece of text as Claude Code sends it: the line Claude still writes shows among the
    /// live rows, below the conversation, until it ends. On a resize, a line taller than the
    /// screen can leave copies of its start in the scrollback, as the README's Resizing tells.
    Chunk,
    /// Each line once it ends, so the live rows hold only the label row and the box, which fit
    /// the screen, and no resize leaves a copy of Claude's text. A long paragraph shows only once
    /// Claude ends it, while the label row says `writing`.
    Line,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            input_rows: NonZeroU16::new(3).expect("3 is not zero"),
            // A cut keeps 44 columns at the start and 43 at the end, which hold a noisy line's
            // marks, tags, tool name and counts whole, since each end's fixed part takes about 20
            max_noisy_line_columns: NonZeroUsize::new(88).expect("88 is not zero"),
            // The width of a noisy line, so Claude's tables and the tools' lines end together
            max_table_columns: NonZeroUsize::new(88).expect("88 is not zero"),
            stream: Stream::Chunk,
            prose_wrap: ProseWrap::Edge,
        }
    }
}

impl Config {
    pub fn read() -> Result<Self, String> {
        let directory = match env::var_os("XDG_CONFIG_HOME") {
            Some(directory) if !directory.is_empty() => PathBuf::from(directory),
            _ => PathBuf::from(env::var_os("HOME").ok_or("HOME is unset")?).join(".config"),
        };
        let path = directory.join("claudio/config.toml");
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Config::default()),
            Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
        };
        toml::from_str(&text).map_err(|error| format!("cannot read {}: {error}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_setting_the_file_leaves_out_keeps_its_default() {
        let config: Config = toml::from_str("input_rows = 5").expect("the file reads");
        assert_eq!(config.input_rows.get(), 5);
        assert_eq!(
            config.max_noisy_line_columns,
            Config::default().max_noisy_line_columns
        );
    }

    #[test]
    fn the_stream_reads_as_a_word() {
        let config: Config = toml::from_str("stream = \"line\"").expect("the file reads");
        assert_eq!(config.stream, Stream::Line);
        assert!(toml::from_str::<Config>("stream = \"paragraph\"").is_err());
    }

    #[test]
    fn the_prose_wrap_reads_as_a_word() {
        let config: Config = toml::from_str("prose_wrap = \"ask-spaces\"").expect("the file reads");
        assert_eq!(config.prose_wrap, ProseWrap::AskSpaces);
        assert!(toml::from_str::<Config>("prose_wrap = \"spaces\"").is_err());
    }

    #[test]
    fn a_setting_of_zero_is_refused() {
        assert!(toml::from_str::<Config>("input_rows = 0").is_err());
    }

    #[test]
    fn an_unknown_setting_is_refused() {
        assert!(toml::from_str::<Config>("input_row = 5").is_err());
    }
}
