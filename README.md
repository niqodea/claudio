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

**Session keys** act on the session. They work the same in every vi mode, and only Ctrl-E changes your text, through your editor:

| Key    | Action                                                   |
| ------ | -------------------------------------------------------- |
| Ctrl-C | stops Claude's turn, or refuses what Claude asks         |
| Ctrl-D | ends the session, when the input is empty                |
| Ctrl-L | clears the context with a new session, keeping your text |
| Ctrl-E | opens your text in `$EDITOR`, or vi when it is unset     |
| Ctrl-O | shows the whole conversation                             |
| Ctrl-S | shows the status                                         |
| Ctrl-Y | opens the questions Claude asks, or goes back from them  |
| Ctrl-N | moves down the options, among the questions             |
| Ctrl-P | moves up the options, among the questions                |

**Every other key works on your text.** Enter sends it, and the rest edit it with vi keys. While a vi command is half typed, its keys show at the right end of the label row. Enter and the session keys still do their usual job then, and cancel the half-typed command. To clear the input, use vi, like `S` or `ggdG` in normal mode; `u` brings it back.

A key that loses what no key brings back waits for a second press in a row, and the label row names the loss in orange, like `enter again refuses 2 questions and sends on a possibly cold cache`.
These keys are:

- Ctrl-D
- Ctrl-L, once the session had a call, which also refuses the questions that wait, or else stops Claude's turn
- Ctrl-C on questions, and while the session ends, where it stops Claude's background tasks too
- Enter on a message while questions wait, which refuses them, or when the cache may be cold, where the call writes the whole context to the cache again, at a price above that of new input: once its time to live passed, and while its age is unknown, as in a resumed session, or before Claude Code tells at startup whether the session is new
- a key that answers a question and leaves out your own text

Ctrl-C stops Claude's turn at once, since a message tells Claude to go on.
Any other key, or a paste, cancels the wait, however long it lasted, and a loss that changes in between waits for one press more.

Ctrl-L starts a new session, as Claude Code's `/clear` does, and keeps your text in the box, ready to be the new session's first message.
The conversation marks the new start with a line like `›› cleared │ session <id>`, which names the session it ended, and `claude --resume <id>` goes back to.
Text you send that starts with `/` reaches Claude as written, so a `/clear` you type is a message like any other.

## Questions

Claude's questions, the choices of `AskUserQuestion`, wait until you open them, so no key meant for your message answers one.
`Y` on the label row counts them, up to 9 and then `+`, in orange while any waits.
Meanwhile, Enter on a message refuses them, so it waits for a second press.
Ctrl-Y shows them one at a time, and Ctrl-Y goes back to the message, as Esc does on the list.
The right end of the label row names the call's questions `a`, `s`, `d` and `f`, green once submitted, with the one shown in reverse.
A question longer than the screen ends in `… Ctrl-O shows it whole`: Ctrl-O there opens the whole conversation at the question, to read with less's keys, and `q` brings you back to answer.

Each question shows its options as numbered lines between the label row and the input box, and last, an item marked `>` for your own answer, which you write in the box.
The cursor starts on the first option.

| Key                  | On the list                                                    |
| -------------------- | -------------------------------------------------------------- |
| `j`/`k`, `↓`/`↑`     | move between the options                                       |
| `1`–`9`              | pick an option, or tick it on several choices                  |
| Space                | tick the option, on several choices                            |
| Enter                | submit the option, or the ticked ones on several choices       |
| `>`                  | write your own answer in the box                               |
| `]`/`[`              | go to the next or previous question                            |
| `a` `s` `d` `f`      | go to that question                                            |
| Esc                  | go back to the message                                         |

The keys that pick an option pick the `>` item too: Enter, or Space on several choices.
Either takes you into the box, at the end of your text, in insert mode, where the vi keys work as in a message.
There, Enter submits your answer, or on several choices, ticks it as one more item and takes you back to the list.
Esc in normal mode takes you back to the list too, keeping the text without ticking it, and `]`/`[` in normal mode go to another question.
Space on your own ticked answer unticks it, keeping the text.
An answer that leaves out the text you wrote waits for the same key again.
Every question needs an answer, so Enter on several choices with none ticked waits too, and to answer none, you write it.
Claude Code instead lets a question go unanswered, which Claude can't tell from a choice of none.
Ctrl-N/Ctrl-P move between the options from anywhere, the box too.

Claude Code takes the answers to a call's questions together, so Enter submits a question and moves on to the next one not submitted.
A submitted question shows its answer in green and takes no keys but Enter, which opens it again, and the keys that leave it.
Once all are submitted, the review lists the answers by their questions' names, all ticked: Enter sends them, or with some unticked, opens those again, as the label row says.
`]` and `[` reach the review too, after the last question.
Claude gets the options' labels and your words, never the numbers, and the conversation shows the answers once sent.
Each question keeps what you picked and typed on it until the call is answered.

## Design

claudio only ever appends to the conversation, in characters on screen, so it copies along with the rest of the scrollback.
Its lines don't depend on the terminal's size, so they read the same at any width, with the terminal wrapping a line wider than the screen.
Below the conversation, the label row and the input box are drawn again on every key and every resize, and never reach the scrollback, so they may use the terminal's size.
The input line's editing state, like vi's mode, may even show without characters, like with the cursor's shape: only the person typing needs it, and only while typing.
The test is whether it would matter to someone reading the scrollback later.
