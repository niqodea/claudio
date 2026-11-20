//! What a resize does to the conversation and to the live rows below it, the label row and the
//! input box, in a terminal with the properties below.
//!
//! # The terminal
//!
//! - Backspace at a row's start goes up only onto a row that wrapped into it, which claudio checks
//!   as it starts, so its climb back to the first live row is exact
//! - Bottom-anchored rewrap: a narrower screen wraps the rows again into more, and shows the last
//!   rows that fit, the empty rows below the cursor included, so the rows it adds push the
//!   screen's top into the scrollback
//! - Empty-first shortening: a shorter screen drops the empty rows below the cursor first
//! - The scrollback is the terminal's for good: no program changes a row of it
//! - Scrollback-first growing: a screen that gains rows takes them from the scrollback's last
//!   rows, at the top, while it holds rows, and adds the rest as empty rows at the bottom
//!
//! tmux 3.8, which these tests run in, has all five.
//!
//! # The screen
//!
//! From the top, the screen holds `a` rows of the conversation, the `h` live rows, and `E` empty
//! rows: `a + h + E` is the screen's height.
//!
//! claudio draws the live rows again once it hears of a resize, and resizes it hears of together
//! act as one, from the size it last drew for to the size it hears of. With `s` rows in the
//! scrollback:
//!
//! - A narrower screen wraps the live rows that fill their row, the label row always, and a row
//!   of the box full of text too, onto `w` more rows, and the bottom-anchored rewrap pushes `w`
//!   rows off the top, while `E` stays: `a` falls by `w`. claudio then draws each live row on one
//!   row again, and the `w` rows become empty: `E` grows by `w`. The conversation's own rows that
//!   wrap again push only themselves off the top.
//! - A wider screen joins the conversation's wrapped rows again, `j` of them, which `a` loses,
//!   and the screen gains back `min(j, s)` from the scrollback: `a` falls by `j - min(j, s)`, and
//!   `E` grows by as many.
//! - A shorter screen takes its rows from `E` first, and from `a` once `E` is 0.
//! - A taller screen adds `t` rows: `a` grows by `min(t, s)`, and `E` by the rest.
//! - Claude's next lines push the live rows down into the empty rows: `a` grows as `E` falls.
//!
//! The live rows leave a copy in the scrollback exactly when their first row goes past the
//! screen's top, beyond any climb: when a narrower screen takes `a` below 0, and when the screen
//! is shorter than `h`, which the line Claude still writes can make taller than any screen. The
//! tests check each rule, and both copies, so a change to claudio or to the terminal that makes a
//! copy fails them, and so does one that lifts a limit the README describes.
//!
//! # Races
//!
//! A resize reaches the terminal at once, but claudio hears of it only later, so what it already
//! sent, drawn for the old size, lands on the new one. In real use this happens all the time on a
//! phone: opening the keyboard shrinks the window while Claude answers. A redraw that relies on
//! where rows were before then erases or repeats rows: in claudio 0.1, a message and the reply that
//! followed it went missing from the scrollback, and copies of the label row were left in it.
//!
//! Timing is what a test cannot set, so these tests set the order of events instead: `pty-relay`
//! sits between the terminal and claudio, holds claudio's output back as if it were still on its
//! way, and tells claudio of the resize only when the test says so. The same order then gives the
//! same screen on every run.
//!
//! They run claudio with the fake Claude Code, on a tmux server of their own.

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
/// The start of the label row once Claude Code told whether the session's cache is warm, before
/// which a message waits for a second Enter, and the label row shows `*` in front of the T.
const KNOWN_CACHE: &str = "›› T";
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

/// The rows of the input box, by default.
const BOX_ROWS: usize = 3;

/// The narrower screens, a column at a time, move the live rows up a row each, until the one
/// that takes them past the screen's top, which leaves one copy of the label row.
#[test]
fn each_narrower_screen_moves_the_live_rows_up_a_row_until_one_leaves_a_copy() {
    let session = Session::start(62, 14);
    session.fill_screen();
    let above = session.rows_above_label();
    for step in 1..=above {
        session.resize(62 - step, 14);
        assert_eq!(
            session.rows_above_label(),
            above - step,
            "after {step} narrower screens"
        );
        assert_eq!(session.label_copies(), 0, "after {step} narrower screens");
    }
    session.resize(61 - above, 14);
    assert_eq!(
        session.label_copies(),
        1,
        "the screen that took the label row past the top"
    );
}

/// One narrower screen moves the live rows up as many rows as the label row wraps onto, however
/// narrower it is, so it leaves no copy while the conversation above has that many rows.
#[test]
fn one_narrower_screen_moves_the_live_rows_up_by_the_rows_the_label_row_wraps_onto() {
    let session = Session::start(62, 14);
    session.fill_screen();
    let above = session.rows_above_label();
    // 62 columns wrap onto 3 more rows at 20
    session.resize(20, 14);
    assert_eq!(session.rows_above_label(), above - 3);
    assert_eq!(session.label_copies(), 0);
}

/// Claude's next lines fill the empty rows that narrower screens left, so the live rows end at
/// the screen's bottom again.
#[test]
fn claudes_next_lines_move_the_live_rows_back_down() {
    let session = Session::start(62, 14);
    session.fill_screen();
    let above = session.rows_above_label();
    for step in 1..=3 {
        session.resize(62 - step, 14);
    }
    assert_eq!(session.rows_above_label(), above - 3);
    session.send_message("message after");
    session.wait_for_reply_to("message after");
    assert_eq!(session.empty_rows_below(), 0);
    assert_eq!(session.label_copies(), 0);
}

/// A shorter screen gives up the empty rows below the live rows first, so the live rows keep
/// their place.
#[test]
fn a_shorter_screen_gives_up_the_empty_rows_first() {
    let session = Session::start(62, 14);
    session.fill_screen();
    let above = session.rows_above_label();
    for step in 1..=3 {
        session.resize(62 - step, 14);
    }
    let empty = session.empty_rows_below();
    assert_eq!(empty, 3);
    session.resize(59, 14 - empty);
    assert_eq!(session.rows_above_label(), above - 3);
    assert_eq!(session.empty_rows_below(), 0);
    assert_eq!(session.label_copies(), 0);
}

/// A wider screen leaves the live rows where they are, empty rows below them included, while the
/// scrollback holds as many rows as join again.
#[test]
fn a_wider_screen_with_a_scrollback_leaves_the_live_rows_where_they_are() {
    let session = Session::start(62, 14);
    session.fill_screen();
    for step in 1..=3 {
        session.resize(62 - step, 14);
    }
    let (above, empty) = (session.rows_above_label(), session.empty_rows_below());
    session.resize(70, 14);
    assert_eq!(session.rows_above_label(), above);
    assert_eq!(session.empty_rows_below(), empty);
}

/// A taller screen takes its rows from the scrollback, above the conversation, and keeps the
/// empty rows below the live rows.
#[test]
fn a_taller_screen_takes_its_rows_from_the_scrollback() {
    let session = Session::start(62, 14);
    session.fill_screen();
    for step in 1..=3 {
        session.resize(62 - step, 14);
    }
    let (above, empty) = (session.rows_above_label(), session.empty_rows_below());
    session.resize(59, 17);
    assert_eq!(session.rows_above_label(), above + 3);
    assert_eq!(session.empty_rows_below(), empty);
}

/// A wider screen with an empty scrollback moves the live rows up by the rows that join again,
/// and leaves as many empty rows below them.
#[test]
fn a_wider_screen_with_an_empty_scrollback_moves_the_live_rows_up() {
    let session = Session::start(30, 30);
    session.send_message("message 1");
    session.wait_for_reply_on_a_narrow_screen("message 1");
    let (above, empty) = (session.rows_above_label(), session.empty_rows_below());
    assert_eq!(session.scrollback_rows(), 0);
    // At 30 columns, 2 of the reply's lines wrap onto a row more each
    session.resize(62, 30);
    assert_eq!(session.rows_above_label(), above - 2);
    assert_eq!(session.empty_rows_below(), empty + 2);
}

/// A taller screen takes as many rows as the scrollback holds from it, and adds the rest as empty
/// rows below the live rows.
#[test]
fn a_taller_screen_adds_empty_rows_once_the_scrollback_runs_out() {
    let session = Session::start(30, 30);
    session.send_message("message 1");
    session.wait_for_reply_on_a_narrow_screen("message 1");
    let above = session.rows_above_label();
    // The empty rows go first, then 2 rows of the conversation, into the scrollback
    let shorter = 30 - session.empty_rows_below() - 2;
    session.resize(30, shorter);
    assert_eq!(session.scrollback_rows(), 2);
    session.resize(30, shorter + 5);
    assert_eq!(session.rows_above_label(), above);
    assert_eq!(session.empty_rows_below(), 3);
    assert_eq!(session.scrollback_rows(), 0);
}

/// Two narrower screens that claudio hears of together act as one, from the size it last drew
/// for: 62 columns wrap onto one more row at 60 as at 61, so the live rows move up a row.
#[test]
fn narrower_screens_heard_together_act_as_one() {
    let session = Session::start(62, 14);
    session.fill_screen();
    let above = session.rows_above_label();
    session.resize_terminal(61, 14);
    session.resize_terminal(60, 14);
    session.tell_size();
    assert_eq!(session.rows_above_label(), above - 1);
}

/// A row of the box full of text fills its row as the label row does, so a narrower screen wraps
/// both, and the live rows move up two rows.
#[test]
fn a_full_row_of_the_box_wraps_as_the_label_row_does() {
    let session = Session::start(62, 14);
    session.fill_screen();
    let above = session.rows_above_label();
    session.type_text(&"x".repeat(100));
    session.resize(61, 14);
    assert_eq!(session.rows_above_label(), above - 2);
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
        session.wait_until("Claude Code tells the cache's state", || {
            session.screen().contains(KNOWN_CACHE)
        });
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

    /// Resizes the terminal, then tells claudio.
    fn resize(&self, width: usize, height: usize) {
        self.resize_terminal(width, height);
        self.tell_size();
    }

    /// Resizes the terminal, which claudio hears of only once told, and waits until the pane's
    /// terminal has the size, since tmux gives it the size after the command returns, and
    /// `pty-relay` passes on the size the terminal has when told.
    fn resize_terminal(&self, width: usize, height: usize) {
        let (columns, rows) = (width.to_string(), height.to_string());
        self.tmux(&["resize-window", "-t", "main", "-x", &columns, "-y", &rows]);
        let terminal = self.tmux(&["display-message", "-p", "-t", "main", "#{pane_tty}"]);
        let pane_height = self.pane("pane_height");
        self.wait_until("the terminal takes the new size", || {
            terminal_size(terminal.trim()) == (width, pane_height)
        });
    }

    /// Tells claudio the terminal's size, and waits until claudio drew for it and the terminal
    /// shows that.
    fn tell_size(&self) {
        let before = self.output().len();
        self.relay("resize");
        self.wait_until("claudio draws for the new size", || {
            self.output().len() > before
        });
        self.wait_until_settled();
    }

    /// Sends messages until the conversation fills the screen, with the live rows at its bottom.
    fn fill_screen(&self) {
        let mut sent = 0;
        while self.empty_rows_below() > 0 {
            sent += 1;
            let message = format!("filler {sent}");
            self.send_message(&message);
            self.wait_for_reply_to(&message);
            self.wait_until_settled();
        }
    }

    /// The conversation's rows on screen above the label row.
    fn rows_above_label(&self) -> usize {
        self.label_row()
    }

    /// The empty rows below the input box, which takes the rows right under the label row.
    fn empty_rows_below(&self) -> usize {
        self.pane("pane_height") - self.label_row() - 1 - BOX_ROWS
    }

    /// The row of the screen, from 0, that the label row is on, which starts with the same marks
    /// however narrow the screen, while its end gives way.
    fn label_row(&self) -> usize {
        (self.screen().lines())
            .position(|line| line.starts_with(KNOWN_CACHE) || line.starts_with("››*T"))
            .expect("the label row is on screen")
    }

    /// The copies of the label row in the scrollback, above the screen.
    fn label_copies(&self) -> usize {
        self.tmux(&["capture-pane", "-p", "-S", "-", "-E", "-1", "-t", "main"])
            .lines()
            .filter(|line| line.contains(LABEL_MARK))
            .count()
    }

    /// The rows in the scrollback, above the screen.
    fn scrollback_rows(&self) -> usize {
        self.pane("history_size")
    }

    fn cursor_row(&self) -> usize {
        self.pane("cursor_y")
    }

    fn pane(&self, variable: &str) -> usize {
        let format = format!("#{{{variable}}}");
        let value = self.tmux(&["display-message", "-p", "-t", "main", &format]);
        value.trim().parse().expect("a number")
    }

    /// Waits until the screen and the cursor stay the same over two looks, so the terminal has
    /// drawn all that claudio wrote.
    fn wait_until_settled(&self) {
        let look = || (self.screen(), self.cursor_row());
        let mut last = look();
        self.wait_until("the screen settles", || {
            thread::sleep(Duration::from_millis(100));
            let now = look();
            let settled = now == last;
            last = now;
            settled
        });
    }

    /// Types `text` into the box, without sending it.
    fn type_text(&self, text: &str) {
        self.tmux(&["send-keys", "-t", "main", "-l", text]);
        self.wait_until_settled();
    }

    fn send_message(&self, text: &str) {
        self.tmux(&["send-keys", "-t", "main", "-l", text]);
        self.tmux(&["send-keys", "-t", "main", "Enter"]);
    }

    fn wait_for_reply_to(&self, message: &str) {
        self.wait_for_reply_ending(message, SECOND_BLOCK);
    }

    /// Waits as `wait_for_reply_to` does, on a screen too narrow for the label row to show `idle`:
    /// until claudio wrote the reply, and the screen settles.
    fn wait_for_reply_on_a_narrow_screen(&self, message: &str) {
        self.wait_until(&format!("the reply to {message}"), || {
            self.after_reply(message, SECOND_BLOCK).is_some()
        });
        self.wait_until_settled();
    }

    /// Waits until claudio wrote the reply to `message` up to `end`, and the label row's `idle`
    /// after it, and tmux shows the label row. It reads what claudio wrote, not what the screen
    /// shows, so a claudio that loses lines on screen fails the test's checks rather than this
    /// wait.
    fn wait_for_reply_ending(&self, message: &str, end: &str) {
        self.wait_until(&format!("the reply to {message}"), || {
            self.after_reply(message, end)
                .is_some_and(|rest| rest.contains(IDLE))
                && self.screen().contains(IDLE)
        });
    }

    /// What claudio wrote from the end of its reply to `message`, once it wrote the reply up to `end`.
    fn after_reply(&self, message: &str, end: &str) -> Option<String> {
        let output = self.output();
        // The message's last copy is in the conversation, since the box empties once it is sent
        let after = &output[output.rfind(message)?..];
        Some(after[after.find(end)?..].to_string())
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

    fn wait_until(&self, what: &str, mut done: impl FnMut() -> bool) {
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

/// The columns and rows of the terminal device at `path`.
fn terminal_size(path: &str) -> (usize, usize) {
    let terminal = fs::File::open(path).expect("the pane's terminal");
    let mut size = libc::winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: the descriptor is open for the call, and `size` is a winsize it may write
    let result = unsafe {
        libc::ioctl(
            std::os::fd::AsRawFd::as_raw_fd(&terminal),
            libc::TIOCGWINSZ,
            &raw mut size,
        )
    };
    assert_eq!(result, 0, "cannot read the terminal's size");
    (usize::from(size.ws_col), usize::from(size.ws_row))
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
