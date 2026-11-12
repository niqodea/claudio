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
}

impl Default for Config {
    fn default() -> Self {
        Config {
            input_rows: NonZeroU16::new(3).expect("3 is not zero"),
            // A cut keeps 44 columns at the start and 43 at the end, which hold a noisy line's
            // marks, tags, tool name and counts whole, since each end's fixed part takes about 20
            max_noisy_line_columns: NonZeroUsize::new(88).expect("88 is not zero"),
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
    fn a_setting_of_zero_is_refused() {
        assert!(toml::from_str::<Config>("input_rows = 0").is_err());
    }

    #[test]
    fn an_unknown_setting_is_refused() {
        assert!(toml::from_str::<Config>("input_row = 5").is_err());
    }
}
