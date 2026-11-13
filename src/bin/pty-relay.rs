//! A stand-in for the wire between a terminal and a program, which tests run between tmux and
//! claudio to set the order of events that a real terminal leaves to timing. It runs the program
//! on a pseudo-terminal of its own, passes the keys typed to it, and passes its output on to the
//! terminal, while it takes commands, one per line, from a named pipe:
//!
//! - `hold`: keep the program's output from the terminal, as if it were still on its way
//! - `release`: pass on what was kept, and from then on pass output on at once
//! - `resize`: give the program the terminal's size now, which signals the program of it
//!
//! The program keeps the size it has until `resize`, whatever the terminal does meanwhile, so a
//! test can resize the terminal while output is held, as happens when output is on its way.
//! All the program's output also goes to a log file, held or not, so a test can wait for what the
//! program wrote, whatever reached the terminal.
//!
//! Usage: `pty-relay <commands> <output-log> <program> [arguments]`

use std::env;
use std::ffi::CString;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::process;
use std::ptr;

use crossterm::terminal;

fn main() {
    let arguments: Vec<String> = env::args().skip(1).collect();
    let [commands, output_log, program @ ..] = arguments.as_slice() else {
        eprintln!("usage: pty-relay <commands> <output-log> <program> [arguments]");
        process::exit(2);
    };
    if program.is_empty() {
        eprintln!("usage: pty-relay <commands> <output-log> <program> [arguments]");
        process::exit(2);
    }
    if let Err(error) = relay(commands, output_log, program) {
        eprintln!("pty-relay: {error}");
        process::exit(1);
    }
}

fn relay(commands: &str, output_log: &str, program: &[String]) -> io::Result<()> {
    let mut size = window_size(libc::STDIN_FILENO)?;
    let mut controller: RawFd = -1;
    // SAFETY: forkpty writes the controller's descriptor and reads the size given, both valid
    let pid = unsafe {
        libc::forkpty(
            &raw mut controller,
            ptr::null_mut(),
            ptr::null(),
            &raw mut size,
        )
    };
    if pid < 0 {
        return Err(io::Error::last_os_error());
    }
    if pid == 0 {
        exec(program);
    }
    terminal::enable_raw_mode()?;
    // Opened for writing too, so the pipe never ends when a writer closes it
    let mut commands = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(commands)?;
    let mut output_log = File::create(output_log)?;
    // SAFETY: forkpty gave the descriptor, which only this File owns from here
    let mut program_side = unsafe { File::from_raw_fd(controller) };
    let mut stdout = io::stdout();
    let mut held: Option<Vec<u8>> = None;
    let mut pending_command = Vec::new();
    let mut buffer = vec![0u8; 65536];
    loop {
        let mut fds = [
            poll_fd(libc::STDIN_FILENO),
            poll_fd(program_side.as_raw_fd()),
            poll_fd(commands.as_raw_fd()),
        ];
        // SAFETY: the array holds three valid entries
        if unsafe { libc::poll(fds.as_mut_ptr(), 3, -1) } < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if fds[0].revents & libc::POLLIN != 0 {
            let read = io::stdin().read(&mut buffer)?;
            program_side.write_all(&buffer[..read])?;
        }
        if fds[1].revents & (libc::POLLIN | libc::POLLHUP) != 0 {
            // The program ended once its side reads nothing, or fails as Linux's does
            let read = match program_side.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => read,
            };
            let output = &buffer[..read];
            if let Some(held) = &mut held {
                held.extend_from_slice(output);
            } else {
                stdout.write_all(output)?;
                stdout.flush()?;
            }
            output_log.write_all(output)?;
        }
        if fds[2].revents & libc::POLLIN != 0 {
            let read = commands.read(&mut buffer)?;
            pending_command.extend_from_slice(&buffer[..read]);
            while let Some(end) = pending_command.iter().position(|&byte| byte == b'\n') {
                let command: Vec<u8> = pending_command.drain(..=end).collect();
                match &command[..end] {
                    b"hold" => held = Some(held.take().unwrap_or_default()),
                    b"release" => {
                        stdout.write_all(&held.take().unwrap_or_default())?;
                        stdout.flush()?;
                    }
                    b"resize" => {
                        let size = window_size(libc::STDIN_FILENO)?;
                        // SAFETY: the size is valid, and the descriptor is the controller's
                        if unsafe {
                            libc::ioctl(program_side.as_raw_fd(), libc::TIOCSWINSZ, &raw const size)
                        } < 0
                        {
                            return Err(io::Error::last_os_error());
                        }
                    }
                    other => {
                        let other = String::from_utf8_lossy(other);
                        return Err(io::Error::other(format!("unknown command {other:?}")));
                    }
                }
            }
        }
    }
    terminal::disable_raw_mode()
}

/// Runs `program` in place of the child, which forkpty set up on the pseudo-terminal.
fn exec(program: &[String]) -> ! {
    let arguments: Vec<CString> = program
        .iter()
        .map(|argument| CString::new(argument.as_str()).expect("arguments hold no NUL"))
        .collect();
    let mut pointers: Vec<*const libc::c_char> =
        arguments.iter().map(|argument| argument.as_ptr()).collect();
    pointers.push(ptr::null());
    // SAFETY: the pointers point into `arguments`, which lives on, and end with a null
    unsafe { libc::execvp(pointers[0], pointers.as_ptr()) };
    eprintln!(
        "pty-relay: cannot run {}: {}",
        program[0],
        io::Error::last_os_error()
    );
    process::exit(127);
}

fn window_size(fd: RawFd) -> io::Result<libc::winsize> {
    let mut size = libc::winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: the size is valid to write
    if unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &raw mut size) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(size)
}

fn poll_fd(fd: RawFd) -> libc::pollfd {
    libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    }
}
