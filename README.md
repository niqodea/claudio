# claudio

Claude Code without the TUI: just input and output.

## Install

```sh
cargo install --path .
```

## Claude Code

claudio drives `claude -p`, and passes its arguments on to `claude`.

claudio supports only auto mode for now: it refuses every permission Claude Code asks for, and says so in red.
It has no key to switch modes either, so set auto mode in `~/.claude/settings.json`:

```json
{ "permissions": { "defaultMode": "auto" } }
```

When the shell has `ANTHROPIC_API_KEY` or `ANTHROPIC_AUTH_TOKEN` set, Claude Code pays with them instead of your subscription.
The interactive Claude asks first, `claude -p` does not, so claudio warns in orange as it starts.
To always use the subscription, add this to `~/.claude/settings.json`, which empties both for every Claude Code session:

```json
{ "env": { "ANTHROPIC_API_KEY": "", "ANTHROPIC_AUTH_TOKEN": "" } }
```

Claude Code's [headless docs](https://code.claude.com/docs/en/headless#start-faster-with-bare-mode) say `--bare` will become the default for `-p` in a future release.
Bare mode ignores the subscription login, as well as CLAUDE.md files, hooks, plugins and auto-memory, so from that release claudio needs whatever turns it back off.

## Config

claudio reads `claudio/config.toml` in `$XDG_CONFIG_HOME`, or in `~/.config` when that is unset, once as it starts.
Every setting is optional, and these are the defaults:

```toml
# The rows of the input box
input_rows = 3
# The columns a detail line takes at most: any line other than your words and Claude's prose
max_detail_columns = 88
```

A detail line wider than that loses its middle to `…`, and Ctrl-O shows it whole.
A setting claudio does not know, or a 0, stops claudio with an error that names it.

## Keys

Keys come in two kinds.

**Session keys** act on the session. They work the same in every vi mode, and none of them changes your text:

| Key    | Action                                                   |
| ------ | -------------------------------------------------------- |
| Ctrl-C | stops Claude's turn, or refuses what Claude asks         |
| Ctrl-D | ends the session, when the input is empty                |
| Ctrl-O | shows the whole conversation                             |
| Ctrl-S | shows the status                                         |
| Ctrl-Y | opens the questions Claude asks, or goes back from them  |
| Ctrl-N | shows the next question, among the questions             |
| Ctrl-P | shows the previous question, among the questions         |

**Every other key works on your text.** Enter sends it, and the rest edit it with vi keys, where `:` in normal mode opens the text in your editor. While a vi command is half typed, its keys show at the right end of the label row. Enter and the session keys still do their usual job then, and cancel the half-typed command. To clear the input, use vi, like `S` or `ggdG` in normal mode; `u` brings it back.

## Questions

Claude's questions, a plan to approve or the choices of `AskUserQuestion`, wait until you open them, so no key meant for your message answers one.
`Y` on the label row counts them, up to 9 and then `+`, in orange while any waits, and Enter sends no message meanwhile.
Ctrl-Y shows them one at a time, with which one of how many at the right end of the label row, and the input box answers the one shown.

The label shows the forms an answer takes, in the notation of a command's usage line:

| Label                       | Answers                                                      |
| --------------------------- | ------------------------------------------------------------ |
| `approve? y, or n [reason]` | `y`, or `n`, or `n` and why                                  |
| `Fruit? a/b/c, or d <own>`  | one key, like `b`, or `d` and your own answer                |
| `Toppings? abcd [more, …]`  | keys run together, like `ac`, then more items apart with commas, or `-` for no key |

Keys apart with `/` take one of them, as a `y/n` prompt does, and keys run together take any of them run together, as a command's flags do, like `ls -la`; `<…>` must follow, and `[…]` may.
Lines below the answer are notes, which Claude reads with it, like the body below a commit message's subject: type `o` in normal mode to start one.
Claude gets the options' labels and your words, never the keys.
An answer that reads any other way answers nothing, and stays in the box, and each question keeps what you typed to it until it is answered.

## Design

claudio only ever appends to the conversation, in characters on screen, so it copies along with the rest of the scrollback.
Its lines don't depend on the terminal's size, so they read the same at any width, with the terminal wrapping a line wider than the screen.
Below the conversation, the label row and the input box are drawn again on every key and every resize, and never reach the scrollback, so they may use the terminal's size.
The input line's editing state, like vi's mode, may even show without characters, like with the cursor's shape: only the person typing needs it, and only while typing.
The test is whether it would matter to someone reading the scrollback later.
