//! What claudio leaves behind when a signal asks it to end: nothing of its own, and it still ends
//! by that signal.

mod common;

use std::fs;
use std::path::Path;
use std::process::Command;

use common::{Tmux, quote};

#[test]
fn kill_ends_claudio_by_its_signal_once_it_removed_its_file() {
    for (name, number) in [("INT", libc::SIGINT), ("TERM", libc::SIGTERM)] {
        let session = Session::start();
        let pid = session.tmux.run(&["display-message", "-p", "#{pane_pid}"]);
        let killed = Command::new("kill")
            .args([&format!("-{name}"), pid.trim()])
            .status()
            .expect("kill runs");
        assert!(killed.success());
        session.tmux.wait_until("claudio ends", || {
            session
                .tmux
                .run(&["display-message", "-p", "#{pane_dead}"])
                .trim()
                == "1"
        });
        let signal = session
            .tmux
            .run(&["display-message", "-p", "#{pane_dead_signal}"]);
        assert_eq!(signal.trim(), number.to_string(), "SIG{name}");
        assert_eq!(session.files(), Vec::<String>::new(), "SIG{name}");
    }
}

#[test]
fn closing_the_terminal_ends_claudio_once_it_removed_its_file() {
    let session = Session::start();
    // The terminal closes with its tmux server, which sends SIGHUP to what runs in it
    session.tmux.run(&["kill-server"]);
    session
        .tmux
        .wait_until("claudio removes its file", || session.files().is_empty());
}

/// claudio, with the fake Claude Code, in a tmux server of its own, and with a temporary
/// directory of its own, where it keeps the file of the full view.
struct Session {
    tmux: Tmux,
    dir: tempfile::TempDir,
}

impl Session {
    /// Starts claudio, and waits until its file is there.
    fn start() -> Self {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let config = dir.path().join("config");
        fs::create_dir(&config).expect("the config directory");
        // exec makes claudio the pane's process, which tmux tells the id and the end of
        let program = format!(
            "exec env TMPDIR={} XDG_CONFIG_HOME={} CLAUDE_CONFIG_DIR={} CLAUDIO_CLAUDE={} {}",
            quote(dir.path()),
            quote(&config),
            quote(&config),
            quote(Path::new(env!("CARGO_BIN_EXE_fake-claude"))),
            quote(Path::new(env!("CARGO_BIN_EXE_claudio"))),
        );
        let session = Session {
            tmux: Tmux::start(80, 24, &program),
            dir,
        };
        // The pane stays once claudio ends, so tmux can tell how it ended
        session.tmux.run(&["set-option", "remain-on-exit", "on"]);
        (session.tmux).wait_until("claudio creates its file", || !session.files().is_empty());
        session
    }

    /// The names of claudio's files in its temporary directory.
    fn files(&self) -> Vec<String> {
        (fs::read_dir(self.dir.path()).expect("the temporary directory"))
            .map(|entry| {
                entry
                    .expect("an entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .filter(|name| name.starts_with("claudio-"))
            .collect()
    }
}
