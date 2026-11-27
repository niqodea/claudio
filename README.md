# claudio

Claude Code without the TUI: just input and output.

## Install

```sh
cargo install --path . --bin claudio
claudio install-claude-code
```

## Claude Code

claudio drives `claude -p`, and takes a few of `claude`'s options, under the same names, which it passes on: `--resume <id>`, `--settings <file>` and `--plugin-dir <dir>`.
Any other argument fails with the usage line, which `--help` shows.
An option keeps `claude`'s name so that it means what it means in `claude`, as [clig.dev](https://clig.dev/#arguments-and-flags) asks to "follow that existing pattern" when another command uses a flag name.
claudio takes only a subset, as `uv pip` takes a subset of `pip`'s options and prioritizes the missing ones "based on user demand" ([uv](https://docs.astral.sh/uv/pip/compatibility/)): it adds an option when someone needs it, once checked with the Claude Code it runs, and an option it passed on unread could break its talk with Claude Code, like `--output-format`.

`--command <command>` runs one of Claude Code's commands as the session starts, written without its `/` as on the [command line](#commands), like `claudio --command "loop 30m"`.
It follows vim's `-c`, which runs a command as vim starts, and has no short form, since `claude -c` continues the last session.

claudio runs its own copy of Claude Code, of the one version it was checked against, since it relies on what is only observed, as [What claudio relies on](#what-claudio-relies-on) tells.
`claudio install-claude-code` downloads that copy into `claudio/claude-code` in `$XDG_DATA_HOME`, or in `~/.local/share` when that is unset, checks its sha256, and removes the copies of other versions.
The copy never updates itself, and a `claude` you run apart from claudio keeps its own version.

To try another Claude Code, name it in `CLAUDIO_CLAUDE`, like `CLAUDIO_CLAUDE=claude claudio`.
When its version is not the one claudio was checked against, claudio warns in orange as it starts.
When Claude Code refuses claudio's request for the context's usage, or answers it in a shape claudio cannot read, claudio stops as it starts, and says why.

`claudio check-claude-code` runs the contract checks against the Claude Code that claudio runs, the one `CLAUDIO_CLAUDE` names too, so it tells whether a newer Claude Code still works with claudio.
The checks that have Claude answer run only with `--paid`, on the cheapest model, at a few cents in all.

claudio has no key to switch the permission mode, which Claude Code's settings set, like auto mode in `~/.claude/settings.json`:

```json
{ "permissions": { "defaultMode": "auto" } }
```

The calls Claude Code asks you about wait among Claude's questions, as [Questions](#questions) tells.

When the shell has `ANTHROPIC_API_KEY` or `ANTHROPIC_AUTH_TOKEN` set, Claude Code pays with them instead of your subscription.
The interactive Claude asks first, `claude -p` does not, so claudio warns in orange as it starts.
To always use the subscription, add this to `~/.claude/settings.json`, which empties both for every Claude Code session:

```json
{ "env": { "ANTHROPIC_API_KEY": "", "ANTHROPIC_AUTH_TOKEN": "" } }
```

claudio runs Claude Code as Anthropic publishes it, and you sign in to it with your own account, through Claude Code's own sign-in.
claudio never reads or stores your login.
Anthropic allows that use of a subscription: its [terms for Claude Code](https://code.claude.com/docs/en/legal-and-compliance#authentication-and-credential-use) let "an end user" sign in "to the unmodified Claude Code binary with their own Claude subscription".

Claude Code's [headless docs](https://code.claude.com/docs/en/headless#start-faster-with-bare-mode) say `--bare` will become the default for `-p` in a future release.
Bare mode ignores the subscription login, as well as CLAUDE.md files, hooks, plugins and auto-memory, so from that release claudio needs whatever turns it back off.

## What claudio relies on

claudio puts each kind of message Claude Code sends in one of three classes:

- it shows the kind, when it matters to you, like Claude's text, a tool call or a compaction
- it skips the kind on purpose, when it does not, like `tool_progress`, Claude Code's progress on a long tool call
- it flags a kind it knows nothing of, like one a newer Claude Code adds, with a `!` line in orange, once per session, and Ctrl+O shows what carried it

A kind can take one class or another by what it holds, like a hook's response, shown only when the hook failed.
`Class` in `src/protocol.rs` gives each kind its class, and each skip its reason.

The kinds claudio names come from what sessions sent, not from a list made in advance: a kind joins them, shown or skipped, once a session sends it and it is flagged.

What claudio shows, it relies on, and each thing it relies on is either promised or observed:

- promised: the types of the Agent SDK of the same version, `sdk.d.ts`, declare it, as Anthropic's published contract for what Claude Code sends
- observed: nothing declares it, so a contract check sees it in the real Claude Code, and the code that relies on it links to the check with `Undocumented:`

One thing claudio reads apart from what Claude Code sends: Claude Code's copy of your plan's usage in its config file, observed by a check like the rest.
Claude Code's answer to `get_usage` leaves out when it fetched its numbers, and falls back to old ones without a word when the usage endpoint fails, while its own `/usage` says so.
The copy holds when it was fetched, by your machine's clock, which brings claudio to parity with `/usage`: where `/usage` warns that its numbers are old, claudio shows the limit as unknown.

Each check runs Claude Code in one of three setups, which trade hermeticity, a result that depends on nothing but the check, for fidelity to your sessions, as [Software Engineering at Google](https://abseil.io/resources/swe-book/html/ch14.html#the_system_under_test) weighs them:

- hermetic: Claude Code runs against a fake of the API on localhost, with a config directory of its own, for a check that needs an answer the API gives only now and then, like a slow one. The check is free, works offline, and gives the same result on every run
- live: Claude Code runs with your login, for a check that needs no answer of Claude's, like one of `/clear`. The check is free, and sees Claude Code as your sessions do
- live and paid: Claude Code runs with your login, and Claude answers with the cheapest model, for a check that needs what only the API gives, like your plan's limits. The check runs only with `--paid`

The fake's Claude follows one plain rule for each kind of request.
It uses a fake rather than canned answers, since a fake behaves like the real API wherever a check meets it, as [Software Engineering at Google](https://abseil.io/resources/swe-book/html/ch13.html#faking-id00043) recommends.
The fake's answers follow the API's docs, and Claude Code is what reads them, so a check fails once Claude Code stops accepting them.

A hermetic check pays for its hermeticity with flags: it sees Claude Code with the defaults of its flags, which may differ from what your sessions do.
Claude Code fetches flags from Anthropic's servers with your login, and they turn its features on and off, like whether a subagent may run in the foreground.
A hermetic check runs it with a config directory of its own and a key the fake makes up, so the fetch fails, and every flag keeps its default.
So a change to a flag on Anthropic's side, which needs no new version of Claude Code, can change what your sessions do while every hermetic check still passes.
A live check sees the flags your sessions get.

What claudio skips, it relies on in no way, so it needs neither.
The flag stays as a last net, since Claude Code sends kinds its types never declare, like `session_title_changed`, and catches what neither the types nor the checks foresaw.
A message that lacks what claudio reads, or holds a value its type has no room for, gets a line in red.

## Tests

The tests come in kinds by what they need to run, as [Software Engineering at Google](https://abseil.io/resources/swe-book/html/ch11.html#test_size) sorts tests by the resources they use, so that most need nothing and give the same result everywhere:

| Kind | Where | Needs | Runs with |
| --- | --- | --- | --- |
| unit | `#[test]` in `src` | nothing | `cargo test` |
| integration | `tests/fake_claude.rs` | the fake Claude Code, which Cargo builds | `cargo test` |
| in a terminal | the other files of `tests` | tmux 3.8 or later | `cargo test --features tmux` |
| against Vim | the comparison of the vi keys in `src/vi.rs` | Vim | `cargo test --features vim` |
| contract | `src/contract.rs` | the Claude Code that claudio runs | `claudio check-claude-code` |

The unit and integration tests are Rust's own two kinds, [test organization](https://doc.rust-lang.org/book/ch11-03-test-organization.html) in its book.
A test that runs a program from outside the repository requires a Cargo feature named after the program, a test file through its [`required-features`](https://doc.rust-lang.org/cargo/reference/cargo-targets.html#the-required-features-field) and a single test by being ignored without it, so `cargo test` needs nothing installed, and `cargo test --all-features` runs all but the contract checks, which [What claudio relies on](#what-claudio-relies-on) tells.

## Config

claudio reads `claudio/config.toml` in `$XDG_CONFIG_HOME`, or in `~/.config` when that is unset, once as it starts.
Every setting is optional, and these are the defaults:

```toml
# The rows of the input box
input_rows = 3
# The columns a noisy line takes at most: a tool call's input or a tool's output
max_noisy_line_columns = 88
# The columns a table of Claude's takes at most as a grid
max_table_columns = 88
# How Claude's text shows while Claude writes it: "chunk", each piece as Claude Code sends it,
# or "line", each line once it ends
stream = "chunk"
# Where the terminal breaks Claude's prose too wide for the screen: "edge", as every terminal
# does, or "ask-spaces", which asks it to break at spaces, as only a patched tmux can so far
prose_wrap = "edge"
```

A noisy line wider than that loses its end to `…`, and Ctrl+O shows it whole.
A wider table shows as a numbered list instead, one item per row with a line for each cell, as psql shows a table too wide for its screen, and it still copies as Markdown.
With `stream = "line"`, a paragraph shows only once Claude ends it, while the label row says `writing`, but no resize leaves a copy of Claude's text in the scrollback, as [Resizing](#resizing) tells.
A setting claudio does not know, or a 0, stops claudio with an error that names it.

## Keys

Keys come in two kinds.

**Session keys** act on the session. They work the same in every vi mode, and only Ctrl+E changes your text, through your editor:

| Key    | Action                                                   |
| ------ | -------------------------------------------------------- |
| Ctrl+C | stops Claude's turn, or refuses what Claude asks         |
| Ctrl+K | stops everything that runs, in the background too        |
| Ctrl+D | ends the session, when the input is empty                |
| Ctrl+L | clears the context with a new session, keeping your text |
| Ctrl+E | opens your text in `$EDITOR`, or vi when it is unset     |
| Ctrl+O | shows the whole conversation                             |
| Ctrl+S | shows the status                                         |
| Ctrl+Y | opens the questions Claude asks, or goes back from them  |
| Ctrl+N | moves down the options, among the questions             |
| Ctrl+P | moves up the options, among the questions                |

**Every other key works on your text.** Enter sends it, and the rest edit it with vi keys. A dim `~` marks each row of the box past your text's end, as vim marks the rows past a buffer's end, so an empty line you typed shows apart from no line at all. The right end of the label row shows in one letter where your keys go, the box's vi mode or the command line: `N` for normal, `I` for insert, `V` for visual and `C` for the command line, and while a vi command is half typed, its keys show just left of it, in 3 columns kept for them, filled from their start as you type, like `d2  N`, and past 3 keys, as vim shows them, each new key pushes out the oldest, like `00d N` for `100d`, so nothing else on the row moves. On a screen too narrow for the whole label row, the label loses its end to `…` and the letter keeps the edge, so it never wraps onto a second row. Enter and the session keys still do their usual job then, and cancel the half-typed command. To clear the input, use vi, like `S` or `ggdG` in normal mode; `u` brings it back.

A key that loses what no key brings back waits for a second press in a row, and the label row names the loss in orange, like `Enter again refuses 2 questions and sends on a possibly cold cache`.
These keys are:

- Ctrl+D
- Ctrl+L, once the session had a call, which also refuses the questions that wait, or else stops Claude's turn
- Ctrl+C on questions
- Ctrl+K, while tasks run in the background
- Enter on a message or a command while questions wait, which refuses them, or when the cache may be cold, where the call writes the whole context to the cache again, at a price above that of new input: once its time to live passed, and while its age is unknown, as in a resumed session, or before Claude Code tells at startup whether the session is new
- a key that answers a question and leaves out your own text

Ctrl+C stops Claude's turn at once, since a message tells Claude to go on.
Any other key, or a paste, cancels the wait, however long it lasted, and a loss that changes in between waits for one press more.

`B` on the label row counts the tasks Claude runs in the background, like a shell, up to 9 and then `+`.
`A` after it counts the subagents that run, in the foreground or in the background, the same way.
Ctrl+C stops Claude's turn and leaves what runs in the background running, and Ctrl+K stops it all: the turn and every task in the background, while the session goes on.
Ctrl+D waits for them to end, and its first press says so, like `Ctrl+D again ends the session after Claude's turn`.
Meanwhile the session takes no text, so the input box and the mode's letter go, and the label row says what the end waits for in place of what Claude does, as a clear that waits does, like `end waits for Claude's turn · Ctrl+C stops`, in orange while background tasks run, which may never end.

Ctrl+L starts a new session, as Claude Code's `/clear` does, and keeps your text in the box, ready to be the new session's first message.
That is why claudio clears rather than leaving it to a restart: you often decide on a fresh context with the message already written, and Ctrl+L sends it there without retyping it.
While tasks run in the background, it waits for them to end before it clears, so their ends, and what Claude does with them, stay in the session that started them, and the label row says so in place of what Claude does, as `clear waits for background tasks · Ctrl+C stops`, in orange, as for the end.
Until the new session starts, Enter and Ctrl+L wait too, since a message sent before can still reach the old one, and claudio refuses the questions Claude asks there, whose turn would hold the clear.
A boundary, `─── cleared ─── · resume with claudio --resume <id>`, ends the old session and starts the new one, and its command resumes the old one, when the old one holds anything to resume.
When claudio ends, the session closes the same way, with `─── ended ───`.
Closing the terminal, or `kill`, ends claudio at once, without that boundary.
Claude Code then finishes its turn and its background tasks on its own, out of view.
Text you send that starts with `/` reaches Claude as written, so a `/clear` you type is a message like any other.

Ctrl+C stops what the label row says the session waits on, and Ctrl+K stops everything; where the two are the same, as while a clear or the end waits, both act, and Ctrl+C does so on its first press, as [clig.dev](https://clig.dev/#signals-and-control-characters) asks of a program that waits for its clean-up.

## Commands

`:` in normal mode opens the command line on the box's last row, as vim's at the screen's bottom, to write one of Claude Code's commands, while your text waits in the rows above it. The row starts with `command:`, a word rather than vim's lone `:`, so it says what it holds even when the box is empty, as Emacs's `M-x` does, and a longer command takes more of the box's rows.
You write the command without its `/`, like `compact` or `model sonnet`.
Tab completes the command's name with the next of the session's commands that start with what you typed, and Shift+Tab with the one before, as on vim's command line, and after the last, Tab brings back what you typed. The commands are those Claude Code lists for the session, its own and your skills, which it tells as the session starts, then again with each session's first message, where a plugin's skills take their plugin's name in front, like `anthropic-skills:pdf`.
The line edits as the box does in insert mode, and having no normal mode, closes on Esc, as on the session keys. Enter runs it.
The conversation marks the command as yours, with a line like `›› /model sonnet`, and Claude Code's answer as its reply, as written, like `Set model to Sonnet 5.5 for this session only`.
claudio handles no command apart from the others, so what one sends shows by its kind, as [What claudio relies on](#what-claudio-relies-on) tells.
`/compact` has no answer: the compaction shows instead, with a line like `compacted from 19,812 to 6,578 tokens`, as it does whenever Claude Code compacts the context.
Claude Code runs a command once Claude's turn ends, and hands one it does not know to Claude, as a message.
Enter waits for a second press as on a message: while questions wait, which a command cancels, and when the cache may be cold, since some commands call the API, like `/compact`, and a mistyped one reaches Claude as a message.

## Questions

Claude asks in three kinds of requests: the questions of a call of `AskUserQuestion`, a plan to carry out, from plan mode, and a call to allow, which Claude Code asks you about when its permission mode and rules leave it to you.
Each request waits until you open it, so no key meant for your message answers one.
`Y` on the label row counts them, a call's questions as one request, up to 9 and then `+`, reversed while any waits.
While any waits, the label row says what the oldest one asks in place of what Claude does, like `asking a question`, `asking 2 questions`, `asking to carry out a plan` or `asking to run Bash`.
Meanwhile, Enter on a message refuses them, so it waits for a second press.
Ctrl+Y shows them one at a time, and Ctrl+Y goes back to the message, as Esc does on the list.
Each part of the screen says one thing about the request.
The title above the question says its kind, `── Question`, `── Plan` or `── Permission`, so it reads the same on every request of a kind, and when more than one request waits, the place of the one shown at its end, like `── Plan ──── 2/7 ──`, each number counted as `Y` counts, up to 9 and then `+`, like `2/+` past the ninth request and `+/+` on a request after it, so the place always takes 3 columns.
The label row says which one it is: a question by its header, a call by its tool, as Claude Code shows it to the user, and a plan as `Plan`, since Claude Code sends no name for it.
The block under the title shows the rest.
The label row gives that name 12 columns, the most Claude Code asks Claude to write in a header, so what follows starts in the same column on every request, and a longer one loses its end.
The right end of the label row shows the progress: the request's questions `a`, `s`, `d` and `f`, green once submitted, when it has more than one, then the requests `q`, `w`, `e` and `r`, always, with the one shown in reverse among each, like `a s · · │ q w e ·`. A request after the fourth has no key, so none of the four shows in reverse on it, and the title gives its place. A group shows a dim `·` for each name it has room for and lacks, like `a s · ·`, and each keeps its columns, so no part moves when another changes. Only the questions can be missing, and they come first, so their empty columns join the room after the label, and no gap opens between parts. A `│` sets apart where your keys go, at the row's end, as between the other groups.
On a narrow screen, each end of the row keeps its edge and gives up text toward the middle, by the screen's width alone: first the progress hides its groups from the left, whole, like `… q w e ·`, then `… │`, so a name never shows cut, then the label loses its end.
Each request goes to Claude Code as soon as you answer it, apart from the others.
A question shows in a block of 8 rows under the title, whatever its length, so the options never move while you answer.
A longer question ends in `… N more lines · Ctrl+O shows it whole`: Ctrl+O there opens the whole conversation at the request, a call's at the call, a quarter down the screen under what led to it, to read with less's keys, and `q` brings you back to answer.
When the options of a question have previews, like code to compare, the question keeps the block's first 3 rows, and the preview of the option under the cursor the last 5, without the code's fences, behind a bar in the margin that starts with the option's key, like `2▎`.
A longer preview ends in the same row that counts the lines left out.

A screen too short for all of it, 18 rows with a box of 3, gives up rows in an order that depends on its height alone, so at any height, every request shows its parts in the same rows.
The box gives up its rows first, down to 1.
Then the block does, from its end: first the rows a value leaves empty, then the rows of the last value of more rows, down to 1, a preview too, whose one row ends in `…`.
Then its last row swallows whole parts from the end, and names what it swallowed: the fields of a call, like `… 2 more fields · Ctrl+O shows them`, and anything else, like a question, or the title too on the last row left, as the request, `… Ctrl+O shows the request`.
So a shorter screen only makes that one row swallow more, and name it more broadly, and Ctrl+O always shows it all.
Below 8 rows, the options give up rows too, and scroll to keep the one under the cursor in view.
Every `…` and every row that counts, here and in the conversation, stands for text that Ctrl+O shows whole.

Each question shows its options as numbered lines between the label row and the input box, and last, an item marked `>` for your own answer, which you write in the box.
The cursor starts on the first option.

| Key                  | On the list                                                    |
| -------------------- | -------------------------------------------------------------- |
| `j`/`k`, `↓`/`↑`     | move between the options                                       |
| `1`–`9`              | move to an option, or tick it on several choices               |
| Space                | tick the option, on several choices                            |
| Enter                | submit the option, or the ticked ones on several choices       |
| `>`                  | write your own answer in the box                               |
| `]`/`[`              | go to the next or previous question                            |
| `a` `s` `d` `f`      | go to that question                                            |
| `}`/`{`              | go to the next or previous request                             |
| `q` `w` `e` `r`      | go to that request                                             |
| Esc                  | go back to the message                                         |

A number only moves the cursor, so you see your choice before Enter answers it.
The keys that pick an option pick the `>` item too: Enter, or Space on several choices.
Either takes you into the box, at the end of your text, in insert mode, where the vi keys work as in a message.
There, Enter submits your answer, or on several choices, ticks it as one more item and takes you back to the list.
Esc in normal mode takes you back to the list too, keeping the text without ticking it, and `]`/`[` in normal mode go to another question.
Space on your own ticked answer unticks it, keeping the text.
An answer that leaves out the text you wrote waits for the same key again.
Every question needs an answer, so Enter on several choices with none ticked waits too, and to answer none, you write it.
Claude Code instead lets a question go unanswered, which Claude can't tell from a choice of none.
Ctrl+N/Ctrl+P move between the options from anywhere, the box too.

Claude Code takes the answers to a call's questions together, so Enter submits a question and moves on to the next one not submitted.
A submitted question shows its answer in green and takes no keys but Enter, which opens it again, and the keys that leave it.
Once all are submitted, the review lists the answers by their questions' names, all ticked: Enter sends them, or with some unticked, opens those again, as the label row says.
A call of one question has nothing to review, so Enter sends its answer at once, and the label row says `Enter sends`.
`]` and `[` reach the review too, after the last question.
Claude gets the options' labels and your words, never the numbers, and the conversation shows the answers once sent.
Each question keeps what you picked and typed on it until the call is answered.
Each request remembers the question you were on, and the review remembers what you unticked, so you find them as you left them when you come back from another request or from your message.

A plan or a call to allow is a request of one question, whose options are named `y` and `n`, as command-line prompts answer a yes-or-no question.
`y` and `n` only move the cursor, as numbers do, and Enter answers: allow allows it, and `refuse, and stop Claude` refuses it and stops Claude's turn, as Ctrl+C does.
Your own answer refuses it too, but Claude reads your words and goes on from them, in the same turn.
A plan shows in the same 8 rows as a question: the plan as written in the first 7, where a longer plan ends in `… N more lines · Ctrl+O shows it whole`, and on the last, who asked, as on a call.
A call shows in the same 8 rows as a question: its input in the first 6, and on the last 2, what the request says about itself, always there, in grey when Claude Code says nothing.
The input shows first the fields that matter most, each after a sign that reads as what it is, then the others, by name.
After `?` comes why Claude Code asks, as it sends it: its words, or else the kind of check that asks, like `A rule in your settings asks before this call`, or else the path that made it ask, and `no reason given` when it sends none of them.
After `↳` comes who asked: the subagent, by its tag and the description of the call that started it, like `#0a Review the diff`, or `main agent`.
A plan can come from a subagent too, one that runs in plan mode, while a question cannot, since Claude Code gives subagents no tool to ask questions.
The signs come from a shell and a diff: Bash shows `$` before its command, `#` before its description, `t` before its timeout and `&` before where it runs, like `& in background`, an Edit shows `-` and `+` before what it takes out and puts in, in red and green, `@` comes before a file or a URL, and `│` before a Write's content or a WebFetch's prompt.
A field the call leaves out shows what that means in grey, like `t default` or `& in foreground`.
A call of another tool, like an MCP tool's, shows each field on a row of its own, after its name, which loses its end past 10 columns, so all its values start in one column.
A call of an MCP tool starts with the server it comes from, as `mcp_server: claude.ai Google Drive`, which says both that the tool is an MCP server's and whose.
Each field takes the same rows on every call of its tool, and a short value leaves them empty, so a field stays in its place from one call to the next, and each value starts in the column its mark sets.
A longer value ends in `… N more lines · Ctrl+O shows it whole`, a value of one row loses its end to `…`, or its middle if it is a file's path or a URL, and the fields that do not fit end in `… N more fields · Ctrl+O shows them`.
A call that Claude Code deems dangerous opens on `n`, as Claude Code's own prompt does, and its option to allow reads `allow (dangerous)`, in orange, and waits for a second Enter, which the label row asks for.
The conversation shows each decision with the call's tag, like `allowed #01 Bash: Run the tests`, then your words, when you gave them.

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
Each line claudio writes in its own words makes sense without the lines next to it, so a line that a search finds, or that is copied alone, still reads whole: the command that resumes a session is on the boundary that ends it.
Only a mark's row, like `››`, and the text a notice quotes under its header depend on the lines next to them.
Its lines don't depend on the terminal's size, so they read the same at any width, with the terminal wrapping a line wider than the screen.
A noisy line, which shows a tool's input or output as it is, is cut at its end to a fixed width, since Ctrl+O shows it whole.
Every other line, like an error or a warning, stays whole, to be read in full.
Below the conversation, the label row and the input box are drawn again on every key and every resize, and reach the scrollback only as [Resizing](#resizing) tells, so they may use the terminal's size.
The input line's editing state, like vi's mode, may even show without characters, like with the cursor's shape: only the person typing needs it, and only while typing.
The test is whether it would matter to someone reading the scrollback later.

Color never carries meaning alone: whatever a color says, a character says too, so the scrollback reads the same without color, as once copied.
A tool call's line, for one, shows Claude's description of the call for a tool that has one, like Bash, and otherwise the call's input as code, in backticks, as Markdown marks literal text.
Ctrl+O shows the whole input, like Bash's command.

`‹‹ summarized` starts a narration, a message Claude wrote to you between tool calls that the API sends only as its own summary, in a thinking block, so you never read Claude's exact words.
Claude Code's interface shows it too, followed by ` · summarized`.
claudio knows a narration by the `narration_block_indexes` Claude Code lists, which nothing documents, so a contract check sees it.

Each tool call's line starts with its tag, like `#0a`, which its result line repeats.

A subagent shows as the call of Claude's that starts it, named by that call's tag, and a line as it ends, which counts its calls, like `✓ #0a Agent: completed · 12 calls`, or `✗` with `failed` or `stopped`.
The call names the subagent's type too when it is not the default `general-purpose`, like `○ #0a Agent: code-reviewer · Review the diff`, since such a subagent runs with a prompt and tools of its own.
Whether a subagent runs in the foreground is Claude Code's choice: the checks saw claudio's Claude Code keep one there when Claude asks for it, while Claude Code's interactive mode sends every subagent to the background.
Its calls and their results show only in the full view, tagged like `#0a#03`, and those of a subagent it starts in turn like `#0a#03#00`, at any depth, since what it did shows in what it reports back to Claude, as in Claude Code, so a search there for `#0a` finds all it did.
Its text stays out, as Claude Code leaves it out unless asked with `--forward-subagent-text`.
A subagent that Claude resumes, with SendMessage, keeps its tag: the call names it, like `○ #0f SendMessage: #0a`, and the line of the resumed run's end is that call's, like `✓ #0f SendMessage: completed · 3 calls`.
claudio knows a subagent by its id alone, which Claude Code puts on all it tells of the subagent, and shows it by its tag everywhere, so a line never names the wrong one.
A message to a subagent by its id shows its tag, like `○ #0f SendMessage: #0a`, and one by the name its Agent call asked for shows the name as written, like `○ #0f SendMessage: reviewer`.
Claude Code reports the name it gave in nothing it sends: the Agent call's result holds the subagent's id alone, as `AgentOutput` in the SDK's [`sdk-tools.d.ts`](https://unpkg.com/@anthropic-ai/claude-agent-sdk@0.3.325/sdk-tools.d.ts) types it, and so does its task's start.
And the name it gave can differ from the one asked for: Claude Code renames a subagent whose name another running one holds, like `reviewer-2`, and frees a name only some time after its holder ended, so a tag found by the name could be another subagent's.
Tools that read Claude Code's hooks miss the name too, as [anthropics/claude-code#100198](https://github.com/anthropics/claude-code/issues/100198) reports, and [#89091](https://github.com/anthropics/claude-code/issues/89091) shows a subagent's type, given in place of its id, failing as an address.
While Claude waits for a call, the label row names its tool and its tag, like `running Bash #03`, and for a subagent in the foreground, counts the others that run in the foreground after it, like `running Agent #0a +2`, and `+N` for 10 or more.
Only the subagents that Claude starts show, since Claude Code does not tell the calls of a subagent that another one starts.

A message you send while Claude waits for a call moves the call's task to the background, a command's or a subagent's alike, where it goes on, and Claude reads your message at once.
A subagent's own calls stay as they are, so its work goes on as it was.

A command that runs in the background, as Claude asks or as a message you send while it runs moves it there, keeps its call's tag to the end.
Its result line says `◌ #00 Bash: running in background`, and once it ends, a line says how, like `✓ #00 Bash: completed`, or `✗` with `failed` or `stopped`.
A call that names the task, like TaskStop's, names it by that tag too.
Ctrl+O shows what Claude Code told Claude of the task, like the file its output goes to, which `tail -f` follows.

## Wording

Every line claudio writes in its own words follows these rules: in the conversation, on the label row, and in its errors.
`src/wording.rs` writes out the grammar of those lines, with the reason for each part.

- Say what happened, in the user's terms, and what they can do about it, when they can do something: `Ctrl+O shows it whole`, not only that a line was cut, as [NN/g](https://www.nngroup.com/articles/error-message-guidelines/) and [clig.dev](https://clig.dev/#errors) ask.
- Put first what the user scans for: the subject of the line, then what it did, like `Claude Code 1.2.4 runs, but 1.2.3 is the one checked to work`, as [Chrome DevTools' guide](https://chromium.googlesource.com/devtools/devtools-frontend/+/main/docs/styleguide/ux/writing.md) asks.
- Name only what the user knows: Claude, Claude Code, the session, the context, a call, a key, and none of claudio's workings, like a kind of message, unless the user can act on it, as [clig.dev](https://clig.dev/#output) asks. claudio never names itself in a session, since it is what the user looks through, but does in a command the user can run, like `claudio --resume <id>`, where its name is part of the command.
- Use one word for one thing, everywhere, the word this README uses: Claude's questions are refused, never denied, as [NN/g](https://www.nngroup.com/articles/consistency-and-standards/) asks.
- Show what a key does where the user looks, while the key matters, so they recognize it rather than recall it, and leave it out otherwise: `Ctrl+C stops` while a clear waits. [NN/g](https://www.nngroup.com/articles/recognition-and-recall/) asks for recognition over recall, and [for shortcuts](https://www.nngroup.com/articles/ui-accelerators/) "readily available, yet easy to ignore".
- Name a key with a capital, and a shortcut with + and no spaces, like Enter, Esc, Space and Ctrl+O, as [Microsoft's style guide](https://learn.microsoft.com/style-guide/a-z-word-list-term-collections/term-collections/keys-keyboard-shortcuts) does, then say what it does from a verb, in 2 to 4 words, like `Enter again sends`, as [NN/g](https://www.nngroup.com/articles/ui-copy/) keeps a command's label.
- Give a command to run after its verb, like `resume with claudio --resume <id>`, so it ends the line, where it is copied whole. [clig.dev](https://clig.dev/#output) asks to suggest the commands to run next, and the order is claudio's own.
- Keep a line to one idea, in the active voice, with plain words and no filler, like please or sorry, as [Chrome DevTools' guide](https://chromium.googlesource.com/devtools/devtools-frontend/+/main/docs/styleguide/ux/writing.md) asks.
- Start with a small letter unless a name starts the line, end with no period, and write numbers as digits, as [Rust's API guidelines](https://rust-lang.github.io/api-guidelines/interoperability.html#c-good-err) and [Chrome DevTools' guide](https://chromium.googlesource.com/devtools/devtools-frontend/+/main/docs/styleguide/ux/writing.md) ask.
- Never let a color alone tell two things apart: say it in words or with a mark too, as [Design](#design) tells. [Primer](https://github.com/primer/design/blob/main/content/native/cli/foundations.mdx#color) uses "color to enhance meaning, not to communicate meaning", and so does [WCAG 1.4.1](https://www.w3.org/WAI/WCAG22/Understanding/use-of-color).

## Resizing

The conversation is printed once, a whole line at a time, and never touched again, so the terminal wraps it again on a resize, as it does any output.
Below it are the live rows: the line Claude is still writing, the label row and the input box, which claudio erases and draws again on every key, every new text and every resize.

The terminal applies a resize at once, but claudio hears of it only later, so what claudio already sent, drawn for the old size, can land on the new one.
So claudio never names a row by its number: each redraw finds its way back to the first live row from the cursor, which the terminal keeps on its character as it wraps the rows again.
As claudio starts, it checks whether a backspace at a row's start goes up only onto a row that wrapped into it, as [tmux's does](https://github.com/tmux/tmux/blob/3.5a/screen-write.c#L983).
Then backspaces stop exactly at the start of each live row, however the terminal wrapped it, and a resize leaves nothing behind, except in the case below.
tmux 3.5 and 3.8 pass the check, and are the only terminals checked so far.
In a terminal that fails it, claudio warns in orange as it starts, and each redraw goes up as many rows as the live rows took.
A narrower screen splits rows into more, so the conversation is never erased, but stray copies of the label row and the box can stay in the scrollback.

The case in tmux: a narrower screen wraps rows into more, and tmux then shows the last rows that fit the screen, the empty ones below the cursor included, so the extra rows push the screen's top into the scrollback even when the screen has empty rows at its bottom ([`grid_reflow`](https://github.com/tmux/tmux/blob/3.5a/grid.c#L1378)).
The label row ends in the mode's letter at the screen's edge, so it fills its row, and any narrower screen wraps it onto one more row, which pushes the screen's top into the scrollback.
claudio then draws the label row on one row again, so the row it took becomes an empty row at the bottom, which the next narrower screen keeps too.
So each narrower screen that claudio draws again after moves the live rows up a row, and once they have moved up as many rows as the screen had above them, a narrower screen pushes the start of the label row out of the screen, where no climb reaches, and leaves a copy of it in the scrollback.
Narrowing a pane a column at a time, like dragging its border or zooming it back into a layout, can do that; one resize, however wide, moves the live rows up only once.
Until the conversation fills the screen, the live rows have few rows above them, so a single narrower screen can be enough.
Claude's next lines move the live rows back down, as they fill the empty rows.
Seen with tmux 3.8: narrowing a 14-row screen a column at a time, 0.4 s apart, moved the live rows up a row each time, and the tenth left a copy.
A fix in tmux would keep the live rows in place: taking the rows a narrower screen adds from the empty rows below the cursor first, as a shorter screen already does.
A shorter screen has no such case, since tmux then drops the empty rows below the cursor first.

One limit holds in any terminal: a program can clear the scrollback only whole, never change a row of it, so live rows taller than the screen push the screen's top into the scrollback for good.
The line Claude is still writing can grow that tall, and each shrink while it does leaves a copy of the rows it pushed out, sometimes with the row `… its start shows once it ends`, which stood in for the line's start.
The line itself still prints once, whole, when it ends, so the scrollback can hold copies of the start of a line above it, but never lacks a line.
With `stream = "line"` in the config, the line Claude is still writing stays out of the live rows, which then hold only the label row and the box. claudio fits those to the screen, so a resize leaves no copy of Claude's text, at the cost of a paragraph showing only once it ends.

No redraw can avoid this, given three things claudio wants:

1. the terminal wraps Claude's lines, so they wrap again on a resize, and copy whole
2. the real cursor stays in the input box
3. a resize never leaves copies in the scrollback

Each pair rules out the third:

- To go on writing a line, claudio needs to get back to the line's end. Once the terminal wraps text again, only a cursor resting there finds it: a backspace finds a line's start, as above, and no sequence finds its end. So with the cursor in the box, the line Claude is still writing stays among the live rows, which can grow taller than the screen: 1 and 2 rule out 3.
- Printing each row of Claude's reply as it fills keeps the live rows short, as [Codex](https://github.com/openai/codex/blob/rust-v0.169.0/codex-rs/tui/src/insert_history.rs) and [oh-my-pi](https://github.com/can1357/oh-my-pi/pull/16117) do, but then claudio breaks the lines itself: 2 and 3 rule out 1.
- A cursor resting at the end of the line Claude writes can always go on from there, but then the box needs a cursor drawn in its place: 1 and 3 rule out 2.

claudio keeps 1 and 2. Claude Code draws its live rows at the bottom of the terminal too, and its users report the same copies on a resize, in [tmux](https://github.com/anthropics/claude-code/issues/52660) and [elsewhere](https://github.com/anthropics/claude-code/issues/51828).

The way out is in the terminal, which applies the resize and so knows where every row went.
kitty, for one, [erases a shell's prompt itself on a resize](https://sw.kovidgoyal.net/kitty/shell-integration/), from the marks of [OSC 133](https://gitlab.freedesktop.org/Per_Bothner/specifications/blob/master/proposals/semantic-prompts.md), so the shell draws it again with no race.
A terminal that did the same for a program's live rows, or that could move the cursor to the end of a wrapped line, would lift the limit.
[Resizes reported in band](https://gist.github.com/rockorager/e695fb2924d36b2bcf1fff4a3704bd83), mode 2048, would not: they reach the program in order with the keys typed, but its output can still be on its way when the terminal resizes.

## Wrapping at spaces

A terminal wraps a line at the screen's edge, so a word can break across two rows.
claudio never breaks a line itself, since a line in the scrollback never depends on the screen's size, as [Design](#design) tells, but it can ask the terminal to.
With `prose_wrap = "ask-spaces"` in the config, claudio asks the terminal as it starts whether it can wrap at spaces, and if it can, starts each line of Claude's prose, a heading, a paragraph, a list item or a quote, with `OSC 2029`, which asks the terminal to wrap that line at spaces instead, again on every resize, and to copy it as one line.
If it cannot, claudio says so in orange and marks nothing, so such a terminal sees only the question.
Only [a patched tmux](https://github.com/niqodea/tmux/tree/niqodea) can so far, kept for claudio's own use rather than maintained for others.

Only prose, since its line breaks mean nothing, so it reads the same wherever it breaks.
Inline code wraps at its spaces like the prose around it, as [github-markdown-css](https://github.com/sindresorhus/github-markdown-css/blob/main/github-markdown.css), a copy of GitHub's style, wraps it with `white-space: break-spaces`.
Keeping inline code on one row would need claudio to mark where the code starts and ends inside the line, besides the `OSC 2029` at the line's start.
A word wider than a row breaks at the edge, as CSS breaks it with [`overflow-wrap: break-word`](https://www.w3.org/TR/css-text-3/#valdef-overflow-wrap-break-word).

In a code block, a line break means something, and a row wrapped at a space ends early, like a line that ended: a long command would look like two.
GitHub's style keeps each line of a code block on one row with [`white-space: pre`](https://www.w3.org/TR/css-text-3/#valdef-white-space-pre) and lets the reader scroll the block sideways, but a terminal's scrollback cannot scroll sideways.
So a code block wraps at the edge, as vim does by default, with [`wrap`](https://vimhelp.org/options.txt.html#%27wrap%27) on and [`linebreak`](https://vimhelp.org/options.txt.html#%27linebreak%27) off: a row wrapped there fills the screen's width, which shows that its line goes on, as vim's [`showbreak`](https://vimhelp.org/options.txt.html#%27showbreak%27) does with a mark a terminal has no way to draw.
Every other line claudio lays out itself, like a table, wraps at the edge too.

claudio writes the spaces of prose as Claude wrote them, as [CSS's `pre-wrap`](https://www.w3.org/TR/css-text-3/#valdef-white-space-pre-wrap) keeps them, rather than showing a run of spaces as one, as a web page shows Markdown.
A run means something in inline code, whose spaces [CommonMark keeps](https://spec.commonmark.org/0.31.2/#code-spans), and in text about spacing, while elsewhere Claude rarely writes one, so showing it as one would gain little.
Inside a row, a run shows as the gap it is, and at a break it hides as the spaces at any line's end do, while a copy keeps it whole.

It is a mark on each line rather than a mode, like autowrap's `?7`, since a mode holds for everything written while it is on: claudio would have to turn it off around the live rows, and a claudio that crashed would leave it on for the shell after it.
A mark holds for its line alone, as the marks of [OSC 133](https://gitlab.freedesktop.org/Per_Bothner/specifications/blob/master/proposals/semantic-prompts.md) hold for a shell's prompt and output, and erasing the line drops it.

It is off by default, since no standard reserves 2029, so claudio's default output stays standard, and a terminal that later gives 2029 another meaning never sees it from claudio unasked.
A sequence few terminals know starts as opt-in: `ls` prints hyperlinks only with [`--hyperlink`](https://www.gnu.org/software/coreutils/manual/html_node/General-output-formatting.html#index-_002d_002dhyperlink), and none is its default.

A mode can be asked about with DECRQM, as [synchronized output](https://github.com/contour-terminal/vt-extensions/blob/master/synchronized-output.md#feature-detection), mode 2026, is, but no general question exists for an OSC.
So `OSC 2029` comes with a question of its own, as [OSC 10 and 11 answer `?`](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html#h3-Operating-System-Commands) with the current color, and `src/terminal.rs` tells how claudio asks it.

## Colors

Each color means one thing, and a character on the line says it too, as [Design](#design) asks:

| Style | Means |
| ----- | ----- |
| The default color | what you and Claude write |
| Bold | headings, strong text, code, tool names, question names and the option under the cursor |
| Dim, or grey on the bar | what stays in the background, like a call's result, whether the call succeeded or failed |
| The bar's background | what is yours: your lines and the input box |
| Cyan | the conversation's marks: `››`, `‹‹`, the rules of a boundary `─── cleared ───`, the cursor `❯` among options and the command line's `command:` |
| Reversed | what's picked out: your selection, the request or question shown, and `Y` and its count while requests wait |
| Orange | what needs your attention: a warning, after `!`, a key that waits for a second press, a cache that turns cold, a limit near its end |
| Red | what failed or was removed: an error that needs you, after `!`, like Claude Code's error output, a limit reached, the lines a diff removes |
| Green | what is done or added: a submitted answer, the lines a diff adds |

Code in a language shows in the colors of its grammar, which your terminal's palette sets.
