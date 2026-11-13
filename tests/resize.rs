//! Resizes that reach the terminal while claudio's output is still on its way to it.
//!
//! In real use this happens all the time on a phone: opening the keyboard shrinks the window
//! while Claude answers, and rotating the phone changes its width. The terminal applies the
//! resize at once, but claudio hears of it only later, so what it already sent, drawn for the
//! old size, lands on the new one. A redraw that relies on where rows were before then erases or
//! repeats rows: in claudio 0.1, a message and the reply that followed it went missing from the
//! scrollback, and copies of the label row were left in it.
//!
//! The race depends on timing, which a test cannot set, so these tests set the order of events
//! instead: `pty-relay` sits between tmux and claudio, holds claudio's output back as if it were
//! still on its way, and tells claudio of the resize only when the test says so. The same order
//! then gives the same screen on every run.
//!
//! They run claudio with the fake Claude Code in tmux, on a tmux server of their own.

use std::ffi::CString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

/// The number of sessions started so far, which gives each session's tmux server its own name.
static SESSIONS: AtomicUsize = AtomicUsize::new(0);

/// How long a step may take before the test gives up on it, far above what any step takes, so
/// that a broken claudio fails the test instead of hanging it.
const STEP_LIMIT: Duration = Duration::from_secs(10);

const LABEL_MARK: &str = "│ Y";
const IDLE: &str = "│ idle";
const FIRST_BLOCK: &str = "The first block of text.";
const SECOND_BLOCK: &str = "The second block, right after the first.";
const LONG_LINE_MESSAGE: &str = "Write a long line";
const LONG_LINE_END: &str = " and that is the end of the long line.";

#[test]
fn a_shrink_while_a_reply_is_on_its_way_loses_nothing_and_leaves_nothing() {
    let session = Session::start(62, 34);
    session.send_message("message 1");
    session.wait_for_reply_to("message 1");

    // The reply to message 2 is drawn for 34 rows, but reaches tmux once it has 14
    session.relay("hold");
    session.send_message("message 2");
    session.wait_until("message 2 is on its way", || {
        session.output().contains("message 2")
    });
    session.tmux(&["resize-window", "-t", "main", "-x", "62", "-y", "14"]);
    session.relay("release");
    session.relay("resize");
    session.wait_for_reply_to("message 2");

    session.send_message("message 3");
    session.wait_for_reply_to("message 3");

    let history = session.history();
    let shown: Vec<&str> = history
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("message ") || [FIRST_BLOCK, SECOND_BLOCK].contains(line))
        .collect();
    let expected: Vec<String> = (1..=3)
        .flat_map(|n| {
            [
                format!("message {n}"),
                FIRST_BLOCK.into(),
                SECOND_BLOCK.into(),
            ]
        })
        .collect();
    assert_eq!(
        shown, expected,
        "the conversation lost or repeated lines:\n{history}"
    );
    let labels = history
        .lines()
        .filter(|line| line.contains(LABEL_MARK))
        .count();
    assert_eq!(labels, 1, "copies of the label row stayed:\n{history}");
}

/// The line Claude is still writing stays among the live rows, so a shrink that makes them taller
/// than the screen pushes the line's first rows into the scrollback, where claudio cannot erase
/// them. The line still prints once, whole, when it ends. The test expects exactly one copy of
/// the line's start: a change that leaves more copies fails it, and so does a change that leaves
/// none, since that removes a limit the README describes.
#[test]
fn a_shrink_below_the_unfinished_line_leaves_one_copy_of_its_start() {
    let session = Session::start(62, 34);
    session.send_message("message 1");
    session.wait_for_reply_to("message 1");

    // The line's 200 words take 17 rows: under 34, over 14
    let start = (1..=200)
        .map(|n| format!("w{n:03}"))
        .collect::<Vec<_>>()
        .join(" ");
    session.send_message(LONG_LINE_MESSAGE);
    session.wait_until("the line's start shows", || {
        session.output().contains("w200") && session.screen().contains("w200")
    });
    session.tmux(&["resize-window", "-t", "main", "-x", "62", "-y", "14"]);
    let before = session.output().len();
    session.relay("resize");
    // claudio draws right after it handles a resize, before it prints more of the reply, so once
    // it writes, the line's end reaches it at 14 rows
    session.wait_until("claudio draws for 14 rows", || {
        session.output().len() > before
    });
    session.open_gate();
    session.wait_for_reply_ending(LONG_LINE_MESSAGE, LONG_LINE_END);

    session.send_message("message 2");
    session.wait_for_reply_to("message 2");

    let history = session.history();
    let whole = format!("{start}{LONG_LINE_END}");
    let lines: Vec<&str> = history.lines().map(str::trim).collect();
    let wholes = lines.iter().filter(|line| **line == whole).count();
    assert_eq!(wholes, 1, "the line is not there once, whole:\n{history}");
    let copies = lines
        .iter()
        .filter(|line| line.starts_with("w001") && **line != whole)
        .count();
    assert_eq!(
        copies, 1,
        "the shrink did not leave one copy of the line's start:\n{history}"
    );
    for message in ["message 1", LONG_LINE_MESSAGE, "message 2"] {
        let count = lines.iter().filter(|line| **line == message).count();
        assert_eq!(count, 1, "{message} is not there once:\n{history}");
    }
    let labels = lines
        .iter()
        .filter(|line| line.contains(LABEL_MARK))
        .count();
    assert_eq!(labels, 1, "copies of the label row stayed:\n{history}");
}

/// claudio in a tmux session of its own, behind `pty-relay`.
struct Session {
    server: String,
    dir: tempfile::TempDir,
}

impl Session {
    fn start(width: u16, height: u16) -> Self {
        let available = Command::new("tmux")
            .arg("-V")
            .output()
            .is_ok_and(|out| out.status.success());
        assert!(
            available,
            "the resize tests run claudio in tmux, which is not installed"
        );
        let dir = tempfile::tempdir().expect("a temporary directory");
        let commands = dir.path().join("commands");
        let gate = dir.path().join("gate");
        make_pipe(&commands);
        make_pipe(&gate);
        fs::create_dir(dir.path().join("config")).expect("the config directory");
        let program = format!(
            "env CLAUDIO_CLAUDE={} FAKE_GATE={} XDG_CONFIG_HOME={} {} {} {} {}",
            quote(&binary("fake-claude")),
            quote(&gate),
            quote(&dir.path().join("config")),
            quote(&binary("pty-relay")),
            quote(&commands),
            quote(&dir.path().join("output")),
            quote(&binary("claudio")),
        );
        let session = Session {
            // Cargo runs the tests in parallel, and each needs a tmux server of its own
            server: format!(
                "claudio-test-{}-{}",
                std::process::id(),
                SESSIONS.fetch_add(1, Ordering::Relaxed)
            ),
            dir,
        };
        let (width, height) = (width.to_string(), height.to_string());
        session.tmux(&[
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
            &program,
        ]);
        session.tmux(&["set-option", "-t", "main", "window-size", "manual"]);
        session.wait_until("claudio starts", || session.screen().contains(LABEL_MARK));
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

    /// Gives `command` to `pty-relay`.
    fn relay(&self, command: &str) {
        let mut pipe = fs::OpenOptions::new()
            .write(true)
            .open(self.dir.path().join("commands"))
            .expect("the relay's pipe");
        writeln!(pipe, "{command}").expect("the relay takes the command");
    }

    /// Lets the fake Claude Code send the end of the long line.
    fn open_gate(&self) {
        let mut pipe = fs::OpenOptions::new()
            .write(true)
            .open(self.dir.path().join("gate"))
            .expect("the fake's gate");
        writeln!(pipe, "go").expect("the fake reads the gate");
    }

    fn send_message(&self, text: &str) {
        self.tmux(&["send-keys", "-t", "main", "-l", text]);
        self.tmux(&["send-keys", "-t", "main", "Enter"]);
    }

    fn wait_for_reply_to(&self, message: &str) {
        self.wait_for_reply_ending(message, SECOND_BLOCK);
    }

    /// Waits until claudio wrote the reply to `message` up to `end`, and the label row's `idle`
    /// after it, and tmux shows the label row. It reads what claudio wrote, not what the screen
    /// shows, so a claudio that loses lines on screen fails the test's checks rather than this
    /// wait.
    fn wait_for_reply_ending(&self, message: &str, end: &str) {
        self.wait_until(&format!("the reply to {message}"), || {
            let output = self.output();
            // The message's last copy is in the conversation, since the box empties once it is sent
            let Some(start) = output.rfind(message) else {
                return false;
            };
            let after = &output[start..];
            after
                .find(end)
                .is_some_and(|reply| after[reply..].contains(IDLE))
                && self.screen().contains(IDLE)
        });
    }

    /// The whole history, with the rows the terminal wrapped joined into their lines.
    fn history(&self) -> String {
        self.tmux(&["capture-pane", "-p", "-J", "-S", "-", "-t", "main"])
    }

    fn screen(&self) -> String {
        self.tmux(&["capture-pane", "-p", "-t", "main"])
    }

    /// All that claudio wrote, whether `pty-relay` held it or passed it on.
    fn output(&self) -> String {
        fs::read(self.dir.path().join("output")).map_or_else(
            |_| String::new(),
            |bytes| String::from_utf8_lossy(&bytes).into_owned(),
        )
    }

    fn wait_until(&self, what: &str, done: impl Fn() -> bool) {
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
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .args(["-L", &self.server, "kill-server"])
            .output();
    }
}

fn make_pipe(path: &Path) {
    let path = CString::new(path.to_str().expect("a UTF-8 path")).expect("no NUL");
    // SAFETY: the path is a valid C string
    assert_eq!(
        unsafe { libc::mkfifo(path.as_ptr(), 0o600) },
        0,
        "cannot make the pipe"
    );
}

fn binary(name: &str) -> PathBuf {
    PathBuf::from(match name {
        "claudio" => env!("CARGO_BIN_EXE_claudio"),
        "fake-claude" => env!("CARGO_BIN_EXE_fake-claude"),
        "pty-relay" => env!("CARGO_BIN_EXE_pty-relay"),
        _ => unreachable!("a binary of this package"),
    })
}

/// `path` quoted for the shell that tmux runs the program with.
fn quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', r"'\''"))
}
