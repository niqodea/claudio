//! The terminal's settings, which the client changes while it runs, and what it can do.

use std::io;
use std::mem::MaybeUninit;
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitStatus};
use std::time::Duration;

use termina::escape::csi::{Csi, Cursor};
use termina::{Event, EventReader, PlatformTerminal, Terminal};

use crate::config::ProseWrap;
use crate::input_line::{cannot_read_terminal, write_out};
use crate::text::Wrap;

/// Asks whether the terminal wraps a line at spaces, `OSC 2029 ; ?`, which a terminal that can
/// answers with `OSC 2029 ; 1`, then for its primary device attributes, `ESC [ c`, which every
/// terminal answers ([xterm]). So a terminal that does not know the question answers only those,
/// and claudio waits for no timeout, as the [kitty keyboard protocol] detects itself.
///
/// [xterm]: https://invisible-island.net/xterm/ctlseqs/ctlseqs.html#h3-Functions-using-CSI-_-ordered-by-the-final-character_s_
/// [kitty keyboard protocol]: https://sw.kovidgoyal.net/kitty/keyboard-protocol/#detection-of-support-for-this-protocol
const ASK_WRAP_AT_SPACES: &[u8] = b"\x1b]2029;?\x1b\\\x1b[c";

/// How long claudio waits for the terminal's next answer, only for a terminal that answers nothing
/// at all.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(2);

/// Hands the client every key as typed until it is dropped: the terminal echoes none, holds none
/// for a whole line, and takes none for itself, like Ctrl+C for a signal, Ctrl+S to pause the
/// output or Ctrl+V to quote the next key. Its output stays as it was, so a line break still
/// starts the next row at its start. Between two readings of the input line, a key then waits
/// for the next one instead of showing up where Claude's text goes. termina reads what the
/// terminal sends, and gives the terminal its original settings back as it is dropped.
pub struct QuietTerminal {
    terminal: PlatformTerminal,
    original: Settings,
}

/// The terminal's settings.
#[derive(Clone, Copy)]
pub struct Settings(libc::termios);

impl QuietTerminal {
    pub fn new() -> Result<Self, String> {
        // Opened first, so that the settings it gives back are the original ones
        let terminal = PlatformTerminal::new().map_err(|error| cannot_read_terminal(&error))?;
        let original = read_settings()
            .map_err(|error| format!("cannot read the terminal's settings: {error}"))?;
        let mut quiet = original.0;
        quiet.c_lflag &= !(libc::ICANON | libc::ECHO | libc::ISIG | libc::IEXTEN);
        quiet.c_iflag &= !(libc::IXON | libc::ICRNL);
        write_settings(Settings(quiet), libc::TCSANOW)
            .map_err(|error| format!("cannot change the terminal's settings: {error}"))?;
        Ok(QuietTerminal { terminal, original })
    }

    /// The settings the terminal had before the client changed them.
    pub fn original(&self) -> Settings {
        self.original
    }

    /// What the terminal sends: keys, pastes, resizes, and its answers to what claudio asks it.
    pub fn events(&self) -> EventReader {
        self.terminal.event_reader()
    }

    /// The terminal's columns and rows.
    pub fn size(&self) -> Result<(u16, u16), String> {
        let size =
            (self.terminal.get_dimensions()).map_err(|error| cannot_read_terminal(&error))?;
        Ok((size.cols, size.rows))
    }

    /// The cursor's column and row, from 0, as the terminal answers when asked. What the terminal
    /// sends meanwhile, like a key, waits in `events` for the input line.
    pub fn cursor_position(&self) -> Result<(u16, u16), String> {
        let is_position = |event: &Event| {
            matches!(
                event,
                Event::Csi(Csi::Cursor(Cursor::ActivePositionReport { .. }))
            )
        };
        write_out(
            Csi::Cursor(Cursor::RequestActivePositionReport)
                .to_string()
                .as_bytes(),
        )?;
        let events = self.events();
        let answered = (events.poll(Some(ANSWER_TIMEOUT), is_position))
            .map_err(|error| cannot_read_terminal(&error))?;
        if !answered {
            return Err("the terminal did not tell where the cursor is".to_string());
        }
        match events.read(is_position) {
            Ok(Event::Csi(Csi::Cursor(Cursor::ActivePositionReport { line, col }))) => {
                Ok((col.get_zero_based(), line.get_zero_based()))
            }
            Ok(_) => unreachable!("the filter lets only the cursor's place through"),
            Err(error) => Err(cannot_read_terminal(&error)),
        }
    }
}

/// Where Claude's prose wraps, as the config's `prose_wrap` asks: at spaces only once the
/// terminal says it can, and at the edge otherwise. The terminal must be quiet.
pub fn prose_wrap(prose_wrap: ProseWrap) -> Result<Wrap, String> {
    match prose_wrap {
        ProseWrap::Edge => Ok(Wrap::Edge),
        ProseWrap::AskSpaces => {
            let can_wrap_at_spaces = wraps_at_spaces()?;
            Ok(if can_wrap_at_spaces {
                Wrap::Spaces
            } else {
                Wrap::Edge
            })
        }
    }
}

/// Whether the terminal says it wraps a line at spaces, asked with `ASK_WRAP_AT_SPACES`. The
/// terminal must be quiet, so that its answers reach claudio as written.
fn wraps_at_spaces() -> Result<bool, String> {
    write_out(ASK_WRAP_AT_SPACES)?;
    let mut answers = Vec::new();
    loop {
        if let Some(wraps) = read_answers(&answers) {
            return Ok(wraps);
        }
        let mut stdin = libc::pollfd {
            fd: libc::STDIN_FILENO,
            events: libc::POLLIN,
            revents: 0,
        };
        let timeout_ms =
            libc::c_int::try_from(ANSWER_TIMEOUT.as_millis()).unwrap_or(libc::c_int::MAX);
        // SAFETY: poll only reads and writes the one pollfd it is given
        match unsafe { libc::poll(&raw mut stdin, 1, timeout_ms) } {
            -1 => return Err(cannot_read_terminal(&io::Error::last_os_error())),
            0 => return Ok(false),
            _ => {}
        }
        let mut buffer = [0u8; 64];
        // SAFETY: read writes at most the buffer's length into the buffer
        let read =
            unsafe { libc::read(libc::STDIN_FILENO, buffer.as_mut_ptr().cast(), buffer.len()) };
        let read = match usize::try_from(read) {
            Ok(0) => return Err(cannot_read_terminal(&io::ErrorKind::UnexpectedEof.into())),
            Ok(read) => read,
            Err(_) => return Err(cannot_read_terminal(&io::Error::last_os_error())),
        };
        answers.extend_from_slice(&buffer[..read]);
    }
}

/// What the terminal's `answers` say once they end with its device attributes, `ESC [ ? … c`:
/// whether `ESC ] 2029 ; 1` came before them.
fn read_answers(answers: &[u8]) -> Option<bool> {
    let start = answers.windows(3).rposition(|bytes| bytes == b"\x1b[?")?;
    let (last, parameters) = answers[start + 3..].split_last()?;
    let is_whole =
        *last == b'c' && (parameters.iter()).all(|byte| byte.is_ascii_digit() || *byte == b';');
    is_whole.then(|| {
        answers[..start]
            .windows(8)
            .any(|bytes| bytes == b"\x1b]2029;1")
    })
}

/// Runs `command` in the terminal and waits for it, as POSIX `system` does. The command runs with
/// the terminal's `original` settings, where Ctrl+C and Ctrl+\ send their signals. The client
/// ignores those signals meanwhile, and the command takes them with their default actions. The
/// client's settings return once the command ends.
pub fn run_in_terminal(original: Settings, command: &mut Command) -> io::Result<ExitStatus> {
    let client = read_settings()?;
    // SAFETY: signal is async-signal-safe, so the child may call it between fork and exec
    unsafe {
        command.pre_exec(|| {
            libc::signal(libc::SIGINT, libc::SIG_DFL);
            libc::signal(libc::SIGQUIT, libc::SIG_DFL);
            Ok(())
        });
    }
    let _interrupt = Ignored::new(libc::SIGINT)?;
    let _quit = Ignored::new(libc::SIGQUIT)?;
    let status = write_settings(original, libc::TCSANOW).and_then(|()| command.status());
    // Restoring the client's settings fails only when the terminal is gone, which leaves the
    // session nothing to show anyway
    let restored = write_settings(client, libc::TCSANOW);
    restored.and(status)
}

/// A signal the client ignores until this is dropped. It then gets back the action it had,
/// whole, with the flags that `signal` would drop, like `SA_SIGINFO` for a handler that reads the
/// signal's details.
struct Ignored {
    signal: libc::c_int,
    previous: libc::sigaction,
}

impl Ignored {
    fn new(signal: libc::c_int) -> io::Result<Self> {
        // SAFETY: an all-zero sigaction is a valid one, with an empty mask and no flags
        let mut ignore: libc::sigaction = unsafe { std::mem::zeroed() };
        ignore.sa_sigaction = libc::SIG_IGN;
        let mut previous = MaybeUninit::<libc::sigaction>::uninit();
        // SAFETY: sigaction reads the new action and fills the previous one when it returns 0
        if unsafe { libc::sigaction(signal, &raw const ignore, previous.as_mut_ptr()) } == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(Ignored {
            signal,
            // SAFETY: sigaction returned 0 above, so it filled the previous action
            previous: unsafe { previous.assume_init() },
        })
    }
}

impl Drop for Ignored {
    fn drop(&mut self) {
        // SAFETY: the action is the one the signal had before, whole
        unsafe { libc::sigaction(self.signal, &raw const self.previous, std::ptr::null_mut()) };
    }
}

fn read_settings() -> io::Result<Settings> {
    let mut settings = MaybeUninit::<libc::termios>::uninit();
    // SAFETY: tcgetattr fills the termios when it returns 0
    if unsafe { libc::tcgetattr(libc::STDIN_FILENO, settings.as_mut_ptr()) } == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: tcgetattr returned 0 above, so it filled the termios
    Ok(Settings(unsafe { settings.assume_init() }))
}

/// Sets the terminal's settings, at the time `when` says, like `TCSANOW`.
fn write_settings(settings: Settings, when: libc::c_int) -> io::Result<()> {
    // SAFETY: the termios is a valid copy of the terminal's settings
    if unsafe { libc::tcsetattr(libc::STDIN_FILENO, when, &raw const settings.0) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    const ATTRIBUTES: &[u8] = b"\x1b[?62;22c";

    #[test]
    fn an_ignored_signal_gets_its_handler_back() {
        // SIGUSR2, which POSIX leaves to applications, so the system never sends it
        let caught = Arc::new(AtomicBool::new(false));
        signal_hook::flag::register(libc::SIGUSR2, Arc::clone(&caught)).expect("a handler");
        drop(Ignored::new(libc::SIGUSR2).expect("ignored"));
        signal_hook::low_level::raise(libc::SIGUSR2).expect("raised");
        assert!(caught.load(Ordering::SeqCst));
    }

    #[test]
    fn the_answers_say_nothing_until_the_device_attributes_end() {
        assert_eq!(read_answers(b"\x1b]2029;1\x1b\\"), None);
        assert_eq!(read_answers(b"\x1b]2029;1\x1b\\\x1b[?62;2"), None);
    }

    #[test]
    fn a_terminal_wraps_at_spaces_only_when_it_says_so_before_its_attributes() {
        assert_eq!(
            read_answers(&[b"\x1b]2029;1\x1b\\", ATTRIBUTES].concat()),
            Some(true)
        );
        assert_eq!(read_answers(ATTRIBUTES), Some(false));
    }
}
