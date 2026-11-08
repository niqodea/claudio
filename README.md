# claudio

Claude Code without the TUI: just input and output.

## Install

```sh
cargo install --path .
```

## Keys

Keys come in two kinds.

**Session keys** act on the session. They work the same in every vi mode, and none of them changes your text:

| Key    | Action                                           |
| ------ | ------------------------------------------------ |
| Ctrl-C | stops Claude's turn, or refuses what Claude asks |
| Ctrl-D | ends the session, when the input is empty        |
| Ctrl-O | shows the whole conversation                     |
| Ctrl-S | shows the status                                 |

**Every other key works on your text.** Enter sends it, and the rest edit it with vi keys, where `:` in normal mode opens the text in your editor. While a vi command is half typed, its keys show at the right end of the label row. Enter and the session keys still do their usual job then, and cancel the half-typed command. To clear the input, use vi, like `S` or `ggdG` in normal mode; `u` brings it back.

## Design

claudio only ever appends to the conversation, in characters on screen, so it copies along with the rest of the scrollback.
Its lines don't depend on the terminal's size, so they read the same at any width, with the terminal wrapping a line wider than the screen.
Below the conversation, the label row and the input box are drawn again on every key and every resize, and never reach the scrollback, so they may use the terminal's size.
The input line's editing state, like vi's mode, may even show without characters, like with the cursor's shape: only the person typing needs it, and only while typing.
The test is whether it would matter to someone reading the scrollback later.
