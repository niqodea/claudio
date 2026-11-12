//! The terminal's settings, which the client changes while it runs.

use std::io;
use std::mem::MaybeUninit;
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitStatus};

/// Hands the client every key as typed until it is dropped: the terminal echoes none, holds none
/// for a whole line, and takes none for itself, like Ctrl+C for a signal, Ctrl+S to pause the
/// output or Ctrl+V to quote the next key. Its output stays as it was, so a line break still
/// starts the next row at its start. Between two readings of the input line, a key then waits
/// for the next one instead of showing up where Claude's text goes.
pub struct QuietTerminal {
    original: Settings,
}

/// The terminal's settings.
#[derive(Clone, Copy)]
pub struct Settings(libc::termios);

impl QuietTerminal {
    pub fn new() -> Result<Self, String> {
        let original = read_settings()
            .map_err(|error| format!("cannot read the terminal's settings: {error}"))?;
        let mut quiet = original.0;
        quiet.c_lflag &= !(libc::ICANON | libc::ECHO | libc::ISIG | libc::IEXTEN);
        quiet.c_iflag &= !(libc::IXON | libc::ICRNL);
        write_settings(Settings(quiet), libc::TCSANOW)
            .map_err(|error| format!("cannot change the terminal's settings: {error}"))?;
        Ok(QuietTerminal { original })
    }

    /// The settings the terminal had before the client changed them.
    pub fn original(&self) -> Settings {
        self.original
    }
}

impl Drop for QuietTerminal {
    fn drop(&mut self) {
        // Restoring the settings can only fail if the terminal is gone, which leaves nothing to
        // restore
        let _ = write_settings(self.original, libc::TCSADRAIN);
    }
}

/// Runs `command` in the terminal and waits for it, as POSIX `system` does: with the terminal's
/// `original` settings, where Ctrl+C and Ctrl+\ send their signals, which the client ignores
/// meanwhile and the command takes with their default actions. The client's settings return
/// once the command ends.
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
    // SAFETY: setting a signal to be ignored runs no code of the client's
    let (interrupt, quit) = unsafe {
        (
            libc::signal(libc::SIGINT, libc::SIG_IGN),
            libc::signal(libc::SIGQUIT, libc::SIG_IGN),
        )
    };
    let status = write_settings(original, libc::TCSANOW).and_then(|()| command.status());
    // Restoring the client's settings fails only when the terminal is gone, which leaves the
    // session nothing to show anyway
    let restored = write_settings(client, libc::TCSANOW);
    // SAFETY: the actions are the ones the client had before
    unsafe {
        libc::signal(libc::SIGINT, interrupt);
        libc::signal(libc::SIGQUIT, quit);
    }
    restored.and(status)
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
