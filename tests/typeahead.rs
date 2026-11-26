//! What the user types before claudio reads the terminal reaches the input box, as POSIX keeps it
//! in the terminal's input queue until a program reads it, even while claudio asks the terminal
//! questions as it starts.

mod common;

use std::fs;
use std::path::Path;

use common::{Tmux, quote};

/// How long the pane waits before it starts claudio, so the keys typed meanwhile wait in the
/// terminal's input queue.
const START_DELAY: &str = "1";

/// What the pane writes once the terminal's echo is off.
const ECHO_OFF: &str = "echo off";

/// With `ask-spaces`, claudio reads the terminal's answers itself as it starts, and with `edge`,
/// only termina reads the terminal.
#[test]
fn keys_typed_before_claudio_starts_reach_the_box() {
    for prose_wrap in ["edge", "ask-spaces"] {
        let session = Session::start(&format!("prose_wrap = \"{prose_wrap}\"\n"));
        session.tmux.wait_until("the terminal's echo is off", || {
            session.tmux.screen().contains(ECHO_OFF)
        });
        session
            .tmux
            .run(&["send-keys", "-t", "main", "-l", "typed ahead"]);
        session.tmux.wait_until(
            &format!("with {prose_wrap}, the keys show in the box"),
            || session.tmux.screen().contains("typed ahead"),
        );
    }
}

/// claudio, with the fake Claude Code and `config`, in a tmux server of its own, started after
/// `START_DELAY` seconds with the terminal's echo off, so only claudio shows what is typed.
struct Session {
    tmux: Tmux,
    _dir: tempfile::TempDir,
}

impl Session {
    fn start(config: &str) -> Self {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let config_dir = dir.path().join("config");
        fs::create_dir_all(config_dir.join("claudio")).expect("the config directory");
        fs::write(config_dir.join("claudio/config.toml"), config).expect("the config file");
        let program = format!(
            "stty -echo; echo {ECHO_OFF:?}; sleep {START_DELAY}; exec env TMPDIR={} XDG_CONFIG_HOME={} \
             CLAUDE_CONFIG_DIR={} CLAUDIO_CLAUDE={} {}",
            quote(dir.path()),
            quote(&config_dir),
            quote(&config_dir),
            quote(Path::new(env!("CARGO_BIN_EXE_fake-claude"))),
            quote(Path::new(env!("CARGO_BIN_EXE_claudio"))),
        );
        Session {
            tmux: Tmux::start(80, 24, &program),
            _dir: dir,
        }
    }
}
