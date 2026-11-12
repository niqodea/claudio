# claudio

Claude Code without the TUI: just input and output.

## Install

```sh
cargo install --path . --bin claudio
claudio install-claude-code
```

## Claude Code

claudio drives `claude -p`, and passes its arguments on to `claude`.

claudio runs its own copy of Claude Code, of the one version it was checked against, since it relies on what is only observed, as [What claudio relies on](#what-claudio-relies-on) tells.
`claudio install-claude-code` downloads that copy into `claudio/claude-code` in `$XDG_DATA_HOME`, or in `~/.local/share` when that is unset, checks its sha256, and removes the copies of other versions.
The copy never updates itself, and a `claude` you run apart from claudio keeps its own version.

To try another Claude Code, name it in `CLAUDIO_CLAUDE`, like `CLAUDIO_CLAUDE=claude claudio`.
When its version is not the one claudio was checked against, claudio warns in orange as it starts.
When Claude Code refuses claudio's request for the context's usage, or answers it in a shape claudio cannot read, claudio stops as it starts, and says why.

`claudio check-claude-code` runs the contract checks against the Claude Code that claudio runs, the one `CLAUDIO_CLAUDE` names too, so it tells whether a newer Claude Code still works with claudio.
The checks that call the API run only with `--paid`, on the cheapest model, at a few cents in all.

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

## What claudio relies on

claudio does one of three things with each kind of message Claude Code sends:

- it shows the kind, when it matters to you, like Claude's text, a tool call or a compaction
- it skips the kind on purpose, when it does not, like `tool_progress`, Claude Code's progress on a long tool call
- it flags a kind it knows nothing of, like one a newer Claude Code adds, with a line in orange, once per session, and Ctrl-O shows what carried it

The kinds claudio names come from what sessions sent, not from a list made in advance: a kind joins them, shown or skipped, once a session sends it and it is flagged.

What claudio shows, it relies on, and each thing it relies on is either promised or observed:

- promised: the types of the Agent SDK of the same version, `sdk.d.ts`, declare it, as Anthropic's published contract for what Claude Code sends
- observed: nothing declares it, so a contract check sees it in the real Claude Code, and the code that relies on it links to the check with `Undocumented:`

What claudio skips, it relies on in no way, so it needs neither.
The flag stays as a last net, since Claude Code sends kinds its types never declare, like `session_title_changed`, and catches what neither the types nor the checks foresaw.
A message that lacks what claudio reads, or holds a value its type has no room for, gets a line in red.

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
- Enter on a message or a command while questions wait, which refuses them, or when the cache may be cold, where the call writes the whole context to the cache again, at a price above that of new input: once its time to live passed, and while its age is unknown, as in a resumed session, or before Claude Code tells at startup whether the session is new
- a key that answers a question and leaves out your own text

Ctrl-C stops Claude's turn at once, since a message tells Claude to go on.
Any other key, or a paste, cancels the wait, however long it lasted, and a loss that changes in between waits for one press more.

Ctrl-L starts a new session, as Claude Code's `/clear` does, and keeps your text in the box, ready to be the new session's first message.
The conversation marks the new start with a line like `›› cleared │ session <id>`, which names the session it ended, and `claude --resume <id>` goes back to.
Text you send that starts with `/` reaches Claude as written, so a `/clear` you type is a message like any other.

## Commands

`:` in normal mode writes one of Claude Code's commands on the label row, as vim's command line, while your text waits in the box.
You write the command without its `/`, like `compact` or `model sonnet`.
The line edits as the box does in insert mode, and having no normal mode, closes on Esc, as on the session keys. Enter runs it.
The conversation marks the command as yours, with a line like `›› /model sonnet`, and Claude Code's answer as its reply, as written, like `Set model to Sonnet 5.5 for this session only`.
claudio handles no command apart from the others, so what one sends shows by its kind, as [What claudio relies on](#what-claudio-relies-on) tells.
`/compact` has no answer: the compaction shows instead, with a line like `compacted from 19,812 to 6,578 tokens`, as it does whenever Claude Code compacts the context.
Claude Code runs a command once Claude's turn ends, and hands one it does not know to Claude, as a message.
Enter waits for a second press as on a message: while questions wait, which a command cancels, and when the cache may be cold, since some commands call the API, like `/compact`, and a mistyped one reaches Claude as a message.

## Questions

Claude's questions, the choices of `AskUserQuestion`, wait until you open them, so no key meant for your message answers one.
`Y` on the label row counts them, up to 9 and then `+`, in orange while any waits.
Meanwhile, Enter on a message refuses them, so it waits for a second press.
Ctrl-Y shows them one at a time, and Ctrl-Y goes back to the message, as Esc does on the list.
The right end of the label row names the call's questions `a`, `s`, `d` and `f`, green once submitted, with the one shown in reverse, when the call has more than one.
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
A call of one question has nothing to review, so Enter sends its answer at once, and the label row says `enter sends`.
`]` and `[` reach the review too, after the last question.
Claude gets the options' labels and your words, never the numbers, and the conversation shows the answers once sent.
Each question keeps what you picked and typed on it until the call is answered.

## Design

claudio takes on something only when it spares a reason to leave for the interactive Claude Code, or when without it, the conversation would hide what happened, as when the context is compacted.
Each need comes from use, never from the docs alone: a feature claudio lacks waits until its absence gets in the way, twice.
One general way in comes before a case of its own: `:` hands any command to Claude Code as written, and what comes back shows as any reply does, unless it would show nothing at all.
claudio need not do everything Claude Code does: for what it lacks, `claude --resume <id>` opens the same session in the interactive Claude Code.

So claudio leaves out on purpose:

- a view of its own for one command, where Claude Code's reply reads well enough
- a key or a list for what Claude Code's settings already set, like the permission mode
- completion and checks that need a list of Claude Code's commands, which would go stale with each new version

claudio only ever appends to the conversation, in characters on screen, so it copies along with the rest of the scrollback.
Its lines don't depend on the terminal's size, so they read the same at any width, with the terminal wrapping a line wider than the screen.
Below the conversation, the label row and the input box are drawn again on every key and every resize, and never reach the scrollback, so they may use the terminal's size.
The input line's editing state, like vi's mode, may even show without characters, like with the cursor's shape: only the person typing needs it, and only while typing.
The test is whether it would matter to someone reading the scrollback later.

Color never carries meaning alone: whatever a color says, a character says too, so the scrollback reads the same without color, as once copied.
A tool call's line, for one, shows Claude's description of the call for a tool that has one, like Bash, and otherwise the call's input as code, in backticks, as Markdown marks literal text.
Ctrl-O shows the whole input, like Bash's command.
