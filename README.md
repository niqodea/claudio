# claudio

Claude Code without the TUI: just input and output.

## Install

```sh
cargo install --path .
```

## Keys

Keys come in two layers.

**Session keys** act on the session. They work the same in every vi mode, and none of them changes your text:

| Key    | Action                                           |
| ------ | ------------------------------------------------ |
| Ctrl-C | stops Claude's turn, or refuses what Claude asks |
| Ctrl-D | ends the session, when the input is empty        |
| Ctrl-O | shows the whole conversation                     |
| Ctrl-S | shows the status                                 |

**Every other key belongs to your text.** Enter sends it, and the rest edit it with vi keys: Esc goes to normal mode, and `:` opens the text in your editor. To clear the input, use vi, like `S` or `ggdG` in normal mode; `u` brings it back.
