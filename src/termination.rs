//! What claudio does with each kind of signal, by the kinds the GNU C Library's manual sorts them
//! into. Their default actions come from POSIX.
//!
//! - Termination signals: SIGHUP, SIGINT and SIGTERM ask claudio to end. The terminal sends
//!   SIGHUP as it closes, and `kill` sends SIGTERM by default. claudio catches all three. It ends
//!   the session as when Claude Code ends, gives the terminal back its settings and modes, and
//!   removes the file of the full view. Then it ends by the same signal, with the signal's default
//!   action, so whoever started it sees which signal ended it.
//! - SIGQUIT keeps its default action. It ends claudio with a core dump, and leaves claudio's
//!   files in place to look at with it.
//! - SIGKILL cannot be caught. It leaves the file of the full view behind.
//! - Program error signals, like SIGSEGV, keep their default actions, since claudio's state may
//!   be broken when one comes.
//! - Job control signals, like SIGTSTP, keep their default actions. No key sends them, nor SIGINT,
//!   since claudio takes every key for itself.
//! - SIGWINCH is termina's, which reads it as a resize.
//! - Alarm, asynchronous I/O and operation error signals keep their default actions. claudio asks
//!   for none of them, and Rust ignores SIGPIPE.
//! - SIGUSR1 and SIGUSR2 keep their default actions, since nothing sends them to claudio.
//!
//! Ref: <https://sourceware.org/glibc/manual/latest/html_node/Standard-Signals.html>, on the kinds
//! of signals
//! Ref: <https://sourceware.org/glibc/manual/latest/html_node/Termination-Signals.html>, on
//! catching termination signals to "delete temporary files, or restore the previous terminal
//! modes", and on leaving the files to SIGQUIT
//! Ref: <https://sourceware.org/glibc/manual/latest/html_node/Termination-in-Handler.html>, on
//! ending by the same signal after cleaning up
//! Ref: <https://pubs.opengroup.org/onlinepubs/9799919799/basedefs/signal.h.html>, on each
//! signal's default action

use std::ffi::c_int;
use std::io;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
use signal_hook::{flag, low_level};

/// The signal that asked claudio to end, if one did.
#[derive(Clone)]
pub struct Termination {
    /// The signal's number, 0 until one came.
    signal: Arc<AtomicUsize>,
}

/// The termination signals claudio catches.
const CAUGHT: [c_int; 3] = [SIGHUP, SIGINT, SIGTERM];

impl Termination {
    /// Catches the `CAUGHT` signals from now on. A caught signal notes its number, then raises
    /// `break_signal`, so whatever wakes on `break_signal` finds the signal noted.
    pub fn catch(break_signal: &Arc<AtomicBool>) -> io::Result<Self> {
        let signal = Arc::new(AtomicUsize::new(0));
        for caught in CAUGHT {
            // signal-hook runs a signal's actions in the order they were registered
            let number = usize::try_from(caught).expect("a signal's number is positive");
            flag::register_usize(caught, Arc::clone(&signal), number)?;
            flag::register(caught, Arc::clone(break_signal))?;
        }
        Ok(Termination { signal })
    }

    /// The signal that asked claudio to end, none until one did.
    pub fn signal(&self) -> Option<c_int> {
        match self.signal.load(Ordering::SeqCst) {
            0 => None,
            signal => c_int::try_from(signal).ok(),
        }
    }
}

/// Ends claudio by `signal`, with the signal's default action, once claudio let go of all it
/// holds. The exit code, a shell's for a program that `signal` ended, is for when the default
/// action cannot run.
pub fn end_by(signal: c_int) -> ExitCode {
    let _ = low_level::emulate_default_handler(signal);
    u8::try_from(128 + signal).map_or(ExitCode::FAILURE, ExitCode::from)
}
