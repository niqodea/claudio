//! What claudio leaves behind when a signal asks it to end: nothing of its own, and it still ends
//! by that signal.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

/// Counts the sessions of this binary, which name their tmux servers apart.
static SESSIONS: AtomicUsize = AtomicUsize::new(0);

/// How long claudio may take to start or to end, far above what it takes.
const STEP_LIMIT: Duration = Duration::from_secs(10);

#[test]
fn kill_ends_claudio_by_its_signal_once_it_removed_its_file() {
    for (name, number) in [("INT", libc::SIGINT), ("TERM", libc::SIGTERM)] {
        let session = Session::start();
        let pid = session.tmux(&["display-message", "-p", "#{pane_pid}"]);
        let killed = Command::new("kill")
            .args([&format!("-{name}"), pid.trim()])
            .status()
            .expect("kill runs");
        assert!(killed.success());
        wait_until("claudio ends", || {
            session
                .tmux(&["display-message", "-p", "#{pane_dead}"])
                .trim()
                == "1"
        });
        let signal = session.tmux(&["display-message", "-p", "#{pane_dead_signal}"]);
        assert_eq!(signal.trim(), number.to_string(), "SIG{name}");
        assert_eq!(session.files(), Vec::<String>::new(), "SIG{name}");
    }
}

#[test]
fn closing_the_terminal_ends_claudio_once_it_removed_its_file() {
    let session = Session::start();
    // The terminal closes with its tmux server, which sends SIGHUP to what runs in it
    session.tmux(&["kill-server"]);
    wait_until("claudio removes its file", || session.files().is_empty());
}

/// claudio, with the fake Claude Code, in a tmux server of its own, and with a temporary
/// directory of its own, where it keeps the file of the full view.
struct Session {
    server: String,
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
            server: format!(
                "claudio-termination-{}-{}",
                std::process::id(),
                SESSIONS.fetch_add(1, Ordering::Relaxed)
            ),
            dir,
        };
        session.tmux(&["-f", "/dev/null", "new-session", "-d", &program]);
        // The pane stays once claudio ends, so tmux can tell how it ended
        session.tmux(&["set-option", "remain-on-exit", "on"]);
        wait_until("claudio creates its file", || !session.files().is_empty());
        session
    }

    fn tmux(&self, arguments: &[&str]) -> String {
        let output = Command::new("tmux")
            .args(["-L", &self.server])
            .args(arguments)
            .output()
            .expect("tmux runs");
        assert!(
            output.status.success(),
            "tmux {arguments:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
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

impl Drop for Session {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .args(["-L", &self.server, "kill-server"])
            .output();
    }
}

/// Waits until `done`, and fails once `what` takes longer than `STEP_LIMIT`.
fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed() < STEP_LIMIT, "{what} did not happen");
        thread::sleep(Duration::from_millis(20));
    }
}

fn quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', r"'\''"))
}
