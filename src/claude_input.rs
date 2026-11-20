//! Claude Code's input, which the session writes its messages and answers to, and the status
//! view its requests for what it shows, until the session closes it.

use std::io::{self, Write};
use std::process::ChildStdin;
use std::sync::{Arc, Mutex};

use crate::lock;
use crate::protocol::Command;

/// One handle to Claude Code's input for each writer, which each writes whole lines through,
/// one at a time.
#[derive(Clone)]
pub struct ClaudeInput(Arc<Mutex<Option<ChildStdin>>>);

impl ClaudeInput {
    pub fn new(input: ChildStdin) -> Self {
        Self(Arc::new(Mutex::new(Some(input))))
    }

    /// Writes `command`, which goes nowhere once the input closed, as Claude Code's exit leaves
    /// it.
    pub fn send(&self, command: &Command) -> Result<(), String> {
        let mut input = lock(&self.0);
        let Some(input) = input.as_mut() else {
            return Ok(());
        };
        match input
            .write_all(command.to_line().as_bytes())
            .and_then(|()| input.flush())
        {
            Err(error) if error.kind() != io::ErrorKind::BrokenPipe => {
                Err(format!("cannot write to claude: {error}"))
            }
            _ => Ok(()),
        }
    }

    /// Closes the input for every handle, as the end of input does a program's in the terminal.
    pub fn close(&self) {
        lock(&self.0).take();
    }
}
