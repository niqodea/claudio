//! How a frame gets from the cursor back to the first live row.
//!
//! The live rows are the rows below the conversation that each frame erases and draws again,
//! from the first one down: the line Claude is still writing, the label row and the input box.
//! The cursor rests in the box between frames.
//!
//! So each frame starts with a climb from the cursor to the first live row. That row cannot be
//! named by its number: the terminal applies a resize the moment it happens, wrapping the rows
//! again and scrolling the screen, while claudio hears of it only later, and a frame already on
//! its way lands on the new screen. A row number from before the resize then points at another
//! row, and the erase that follows can take the conversation's last lines with it.
//!
//! The climb rather moves relative to the cursor, which the terminal keeps on its character when
//! it wraps the rows again, as tmux does ([`screen_reflow`]). Each live row is drawn as a line of
//! its own, after a line break, and a frame climbs it in one of two ways:
//!
//! - [`Climb::Exact`] backspaces along the line. A backspace at a row's first column moves to the
//!   end of the row above only when that row wrapped into this one, as tmux's does
//!   ([`screen_write_backspace`]), so enough backspaces stop exactly at the line's start however
//!   the terminal wrapped it. One row up from there is the previous line's last row, which the
//!   climb goes on from. Nothing on the way depends on the screen's size, so a resize at any
//!   moment, even in the middle of a frame, cannot move where the climb ends.
//! - [`Climb::Counted`] goes up as many rows as the live rows took when drawn. A narrower screen
//!   splits rows, so the count can fall short, and leave the rows it misses in the scrollback,
//!   but never overshoot, since claudio draws no row wider than the screen and a wider one joins
//!   none. It needs nothing of the terminal.
//!
//! Which way works is a matter of the terminal, so [`check`] tries it once, as claudio starts:
//! it writes a line that wraps, backspaces, and asks where the cursor ended, as one checks how
//! wide a terminal draws a character by printing it and asking ([wcwidth]). Asking whether the
//! terminal supports a mode cannot tell, since tmux has no mode for it, and xterm's mode of it
//! ([reverse-wraparound], mode 45) may also backspace across a line break.
//!
//! Either way, live rows taller than the screen push the screen's top into the scrollback,
//! beyond any climb: the README's Resizing tells when that happens, and why no redraw avoids it.
//!
//! [`screen_reflow`]: https://github.com/tmux/tmux/blob/3.5a/screen.c#L586
//! [`screen_write_backspace`]: https://github.com/tmux/tmux/blob/3.5a/screen-write.c#L983
//! [wcwidth]: https://pypi.org/project/wcwidth/
//! [reverse-wraparound]: https://invisible-island.net/xterm/ctlseqs/ctlseqs.html

use crossterm::cursor;
use crossterm::terminal;

use crate::input_line::{cannot_read_terminal, write_out};

/// Lets a backspace at a row's first column move to the row above, in terminals like xterm that
/// keep it off by default. Others, tmux among them, have no such mode and ignore it.
const BACKSPACE_WRAPS_ON: &str = "\x1b[?45h";
pub const BACKSPACE_WRAPS_OFF: &str = "\x1b[?45l";

const UP: &str = "\x1b[A";

/// Erases the cursor's row, with the cursor at its first column, and all the rows below. It
/// erases below from the row's second column, then the row whole, since tmux takes an erase
/// below from the screen's top-left corner for clearing the screen, and by default moves the
/// screen's rows to the scrollback instead, as its `scroll-on-clear` option says
/// ([`screen_write_clearendofscreen`]). The first live row is at that corner whenever the
/// conversation has not reached the screen yet, as on a cleared screen.
///
/// [`screen_write_clearendofscreen`]: https://github.com/tmux/tmux/blob/3.5a/screen-write.c#L1540
pub const ERASE_DOWN: &str = "\x1b[C\x1b[J\r\x1b[K";

/// How a frame gets from the cursor back to the first live row.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Climb {
    /// Backspaces along each line, which stop at its start.
    Exact,
    /// Goes up as many rows as the live rows took, which may fall short, never overshoot.
    Counted,
}

/// Writes to `out` the climb from the cursor to the first column of the line `lines` lines above
/// the cursor's, where each line is a row the last frame drew on a screen `width` wide.
pub fn write(out: &mut Vec<u8>, climb: Climb, lines: u16, width: u16) {
    match climb {
        Climb::Exact => {
            backspace_to_line_start(out, width);
            for _ in 0..lines {
                out.extend(format!("{UP}\r").as_bytes());
                backspace_to_line_start(out, width);
            }
        }
        Climb::Counted => {
            out.push(b'\r');
            if lines > 0 {
                out.extend(format!("\x1b[{lines}A").as_bytes());
            }
        }
    }
}

/// Writes the backspaces that take the cursor to the start of its line, a row drawn on a screen
/// `width` wide, wherever the terminal wrapped it since. The row holds at most `width` cells, and
/// a narrower screen splits it into rows that are full but for at most one cell each, which a
/// wide character too wide for the rest of a row leaves empty. So the cursor is at most twice
/// `width` backspaces from the line's start, and those left over stop there.
fn backspace_to_line_start(out: &mut Vec<u8>, width: u16) {
    out.extend(std::iter::repeat_n(b'\x08', 2 * usize::from(width)));
}

/// Checks whether [`Climb::Exact`] works in this terminal, from the cursor, which must be at the
/// first column of a row that a line break starts, with a row below it.
///
/// It writes one space more than fit on the row, which wraps the last one onto the row below,
/// backspaces more than enough to get back, and asks where the cursor is. The cursor is back
/// where it started when backspaces climb wrapped rows and stop at a line break. It stays on the
/// row below in a terminal whose backspace stops at a row's start, and climbs past the start in
/// one whose backspace crosses line breaks too. Then it erases the spaces, and leaves the cursor
/// where it started.
///
/// A resize meanwhile wraps the spaces again, so the check runs again until the screen's size
/// stays the same throughout. It fails when the terminal does not tell the cursor's place in
/// time.
pub fn check() -> Result<Climb, String> {
    loop {
        let size = terminal::size().map_err(|error| cannot_read_terminal(&error))?;
        let climb = check_once(size.0)?;
        if terminal::size().map_err(|error| cannot_read_terminal(&error))? == size {
            return Ok(climb);
        }
    }
}

fn check_once(width: u16) -> Result<Climb, String> {
    let (_, start) = cursor::position().map_err(|error| cannot_read_terminal(&error))?;
    let mut out = Vec::new();
    out.extend(BACKSPACE_WRAPS_ON.as_bytes());
    out.extend(" ".repeat(usize::from(width) + 1).as_bytes());
    backspace_to_line_start(&mut out, width);
    write_out(&out)?;
    let (column, row) = cursor::position().map_err(|error| cannot_read_terminal(&error))?;
    let climb = if (column, row) == (0, start) {
        Climb::Exact
    } else {
        Climb::Counted
    };
    let mut out = Vec::new();
    if row > start {
        out.extend(format!("\x1b[{}A", row - start).as_bytes());
    } else if row < start {
        out.extend(format!("\x1b[{}B", start - row).as_bytes());
    }
    // The spaces take the start row and the one below, which nothing else was on
    out.push(b'\r');
    out.extend(ERASE_DOWN.as_bytes());
    if climb == Climb::Counted {
        out.extend(BACKSPACE_WRAPS_OFF.as_bytes());
    }
    write_out(&out)?;
    Ok(climb)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn written(climb: Climb, lines: u16, width: u16) -> String {
        let mut out = Vec::new();
        write(&mut out, climb, lines, width);
        String::from_utf8(out).expect("the climb is ASCII")
    }

    #[test]
    fn an_exact_climb_backspaces_along_each_line_and_goes_up_between_them() {
        let backspaces = "\x08".repeat(6);
        assert_eq!(
            written(Climb::Exact, 2, 3),
            format!("{backspaces}{UP}\r{backspaces}{UP}\r{backspaces}")
        );
    }

    #[test]
    fn a_counted_climb_goes_up_the_rows_drawn() {
        assert_eq!(written(Climb::Counted, 2, 80), "\r\x1b[2A");
        assert_eq!(written(Climb::Counted, 0, 80), "\r");
    }
}
