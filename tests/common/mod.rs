//! A tmux server for a test to run claudio in, which the test drives and reads.

use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

/// Counts the servers of this test binary, which name them apart.
static SERVERS: AtomicUsize = AtomicUsize::new(0);

/// How long a step may take before the test gives up on it, far above what any step takes.
const STEP_LIMIT: Duration = Duration::from_secs(10);

/// A tmux server of its own, since Cargo runs the tests in parallel, with one session, `main`.
/// Dropping it kills the server.
pub struct Tmux {
    server: String,
}

impl Tmux {
    /// Starts the server, with `main` `width` columns wide and `height` rows high, running
    /// `program` with the shell.
    pub fn start(width: u16, height: u16, program: &str) -> Self {
        let available = Command::new("tmux")
            .arg("-V")
            .output()
            .is_ok_and(|out| out.status.success());
        assert!(
            available,
            "the test runs claudio in tmux, which is not installed"
        );
        let tmux = Tmux {
            server: format!(
                "claudio-test-{}-{}",
                std::process::id(),
                SERVERS.fetch_add(1, Ordering::Relaxed)
            ),
        };
        let (width, height) = (width.to_string(), height.to_string());
        tmux.run(&[
            "-f",
            "/dev/null",
            "new-session",
            "-d",
            "-s",
            "main",
            "-x",
            &width,
            "-y",
            &height,
            program,
        ]);
        tmux
    }

    /// Runs the tmux command `arguments` on the server, and returns what it printed.
    pub fn run(&self, arguments: &[&str]) -> String {
        let output = self.output(arguments);
        assert!(
            output.status.success(),
            "tmux {arguments:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    #[allow(dead_code, reason = "not every test reads the screen")]
    pub fn screen(&self) -> String {
        self.run(&["capture-pane", "-p", "-t", "main"])
    }

    /// The whole history, with the rows the terminal wrapped joined into their lines, and empty
    /// once the server is gone.
    pub fn history(&self) -> String {
        let output = self.output(&["capture-pane", "-p", "-J", "-S", "-", "-t", "main"]);
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// Waits until `done`, and fails with the history once `what` takes longer than
    /// `STEP_LIMIT`.
    pub fn wait_until(&self, what: &str, mut done: impl FnMut() -> bool) {
        let start = Instant::now();
        while !done() {
            assert!(
                start.elapsed() < STEP_LIMIT,
                "{what} did not happen. The history:\n{}",
                self.history()
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn output(&self, arguments: &[&str]) -> std::process::Output {
        Command::new("tmux")
            .args(["-L", &self.server])
            .args(arguments)
            .output()
            .expect("tmux runs")
    }
}

impl Drop for Tmux {
    fn drop(&mut self) {
        let _ = self.output(&["kill-server"]);
    }
}

/// `path` quoted for the shell that tmux runs the program with.
pub fn quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', r"'\''"))
}
