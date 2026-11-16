# claudio

The README tells how claudio works and why. These are the rules for changing it.

## Wording

Every line claudio writes follows these rules: in the conversation, on the label row, and in its errors.
Each rule names its source, or says it is claudio's own.

- Say what happened, in the user's terms, and what they can do about it, when they can do something: `Ctrl+O shows it whole`, not only that a line was cut. [NN/g](https://www.nngroup.com/articles/error-message-guidelines/), [clig.dev](https://clig.dev/#errors)
- Put first what the user scans for: the subject of the line, then what it did, like `Claude Code 2.1.330 runs, but 2.1.322 is the one checked to work`. [Chrome DevTools](https://chromium.googlesource.com/devtools/devtools-frontend/+/main/docs/styleguide/ux/writing.md)
- Name only what the user knows: Claude, Claude Code, the session, the context, a call, a key, and none of claudio's workings, like a kind of message, unless the user can act on it. [clig.dev](https://clig.dev/#output). claudio never names itself in a session, since it is what the user looks through: claudio's own rule
- Use one word for one thing, everywhere, the word the README uses: Claude's questions are refused, never denied. [NN/g](https://www.nngroup.com/articles/consistency-and-standards/)
- Name a key with a capital, and a shortcut with + and no spaces, like Enter, Esc, Space and Ctrl+O, then say what it does: `Enter again sends`. [Microsoft](https://learn.microsoft.com/style-guide/a-z-word-list-term-collections/term-collections/keys-keyboard-shortcuts)
- Keep a line to one idea, in the active voice, with plain words and no filler, like please or sorry. [Chrome DevTools](https://chromium.googlesource.com/devtools/devtools-frontend/+/main/docs/styleguide/ux/writing.md)
- Start with a small letter unless a name starts the line, end with no period, and write numbers as digits. [Rust](https://rust-lang.github.io/api-guidelines/interoperability.html#c-good-err), [Chrome DevTools](https://chromium.googlesource.com/devtools/devtools-frontend/+/main/docs/styleguide/ux/writing.md)
- Say a warning in words too, never by its color alone. [WCAG 1.4.1](https://www.w3.org/WAI/WCAG22/Understanding/use-of-color)

## Color

- Style text only by a meaning from `src/style.rs`, like `SUBDUED` or `ATTENTION`, never by a raw code, which is private to it. Its names are the rows of the README's Colors table, so a new meaning, or a new use of a color, goes into the table and gets its name there first: claudio's own rule
- Give what stays in the background one style wherever it shows: `SUBDUED`, and `SUBDUED_ON_BAR` on the bar alone, where dim text would not read. A mark, like a field's sign, takes its style from what it marks, never from the value after it: claudio's own rule
- Keep red for what failed. [clig.dev](https://clig.dev/#output)
- Set code in bold, as man pages set literal text. [man-pages](https://man7.org/linux/man-pages/man7/man-pages.7.html#FORMATTING_AND_WORDING_CONVENTIONS)
- Prefer the terminal's 16 colors, which its theme sets, to fixed shades, and keep fixed shades for what the 16 lack: orange, and the bar's subtle background and the grey on it. [bat](https://github.com/sharkdp/bat#8-bit-themes)
- Keep text at a contrast of at least 4.5:1 with what is behind it, the bar included. [WCAG 1.4.3](https://www.w3.org/WAI/WCAG22/Understanding/contrast-minimum)

## Cutting

- Cut only a noisy line, which shows a tool's input or output as it is, and keep every other line in the conversation whole, like an error, a warning or Claude's description of a call: claudio's own rule, whose reason the README's Design section gives
- Cut a line at its end, with `…`, since text reads from its start and the eye finds an `…` that is always in the same place, and keep the whole line in the full view: `cut`. [PatternFly](https://www.patternfly.org/components/truncate/design-guidelines)
- Cut a file's path or a URL in the middle instead, with `…`, since paths often start the same and their last part is the file or the page: `cut_path`. [PatternFly](https://www.patternfly.org/components/truncate/design-guidelines), [PatternFly](https://v5-archive.patternfly.org/ux-writing/truncation)
- On the question screen, cut each line at the screen's width, since a line there takes exactly one row: claudio's own rule
- Leave a sign wherever a cut hides text, and only empty rows may go without one, since they hide nothing: `…` at the end of a line, or a row that counts what it hides, like `… 2 more fields · Ctrl+O shows them`. [PatternFly](https://v5-archive.patternfly.org/ux-writing/truncation), which marks a cut with `…`
- Show whole, in a view the user can open, all that a cut hides, and give each cut the view that does, as `cut::Whole`, so no cut hides text for good. [WCAG 1.4.10](https://www.w3.org/WAI/WCAG22/Understanding/reflow.html), which allows a cut while a mechanism reveals the text, [PatternFly](https://v5-archive.patternfly.org/ux-writing/truncation), which asks for at least one way to see the whole text
- Name the key of that view on a row that counts, which has the room, and leave an `…` without one, since the line it ends has none left: an `…` stands for the full view, Ctrl+O. PatternFly's `…` names no way to see the text either, since its tooltip on hover is known to all, while a terminal has no such custom, so a row that can name the key does: claudio's own rule
- Cut every line and row in `src/cut.rs`, so the rules above hold in one place: claudio's own rule

## Height

- Give each part of a screen its rows by the screen's height alone, never by what it shows, so no part moves from one request to the next: claudio's own rule, the same one the label row keeps for width
- On a short screen, take rows first from the box, then from the block under the title, which shows the least needed to answer, then from the options, and keep to the last the label row, the user's own answer and a row of the box, with which the user answers: `ViewRows`. [fzf](https://github.com/junegunn/fzf/blob/master/src/terminal.go), whose preview gives up its rows before its list and prompt
- Within the block, cut from the end, as a list reads from its start: empty rows first, then the rows of the last value of more rows, then the last fields, counted on a row: `cut::fit`. Let what is left take the room of what was cut. [fzf](https://github.com/junegunn/fzf/blob/master/src/terminal.go), whose list takes the rows its hidden preview leaves
- Hide whole parts into one row at the stack's end, which swallows more as the screen shrinks, the title last, and names what it holds as closely as one of the README's words can: `N more fields` for a call's fields, and else `the request`. The user learns a single row, always in the same place and always naming the key that shows it all, rather than a sign for each height: `cut::fit` and `stack_lines`. [Primer](https://primer.style/product/components/underline-nav), whose overflow menu takes the items that do not fit one by one, [Priority+](https://css-tricks.com/the-priority-navigation-pattern/), whose "more" holds what the screen has no room for, [PatternFly](https://www.patternfly.org/components/overflow-menu/design-guidelines), whose actions collapse into one menu as the screen shrinks
- Mark a preview with a bar in its margin, which starts with its option's key, like `2▎`, rather than with a title row, so it is a value like any other, which gives up rows down to 1 and keeps its sign at its end: `Mark::Quote`. [synaps-tui](https://docs.rs/crate/synaps-tui/0.3.3/source/src/tui/render.rs), which sets its tool blocks apart with a bar in their margin
- Keep the option under the cursor in view, scrolling the options when there are more of them than rows. [WCAG 2.4.11](https://www.w3.org/WAI/WCAG22/Understanding/focus-not-obscured-minimum.html)

## Label row

- Put each indicator in its group, with `│` between groups: the budgets, which tell how much room is left, as `T`, `S` and `W`, then the activity, which tells what happens now, as `Y`, `B` and what Claude does, `idle` included. Items close together read as one group, and space or a line sets groups apart. [NN/g](https://www.nngroup.com/articles/gestalt-proximity/)
- Always show what Claude does, `idle` too, since the user decides their next step by it. [NN/g](https://www.nngroup.com/articles/visibility-system-status/)
- Within a group, put first what needs the user, and among the budgets, the narrowest first, since attention leans left. [NN/g](https://www.nngroup.com/articles/horizontal-attention-leans-left/)
- Keep what holds for the whole session on the left, and what holds for the moment, like a half-typed vi command, at the right end. [VS Code](https://code.visualstudio.com/api/ux-guidelines/status-bar)
- Keep each indicator's width fixed, with one character for a count, so it stays where the eye learned to find it, and put text of varying width last: claudio's own rule
- On the question screen, give the question's header, or a call's tool, 12 columns, padded or cut, the most Claude Code asks Claude to write in a header, so the hint after it stays in place: claudio's own rule, by the limit in `AskUserQuestionInput` of `sdk-tools.d.ts`

## Question screen

- Give each part of the screen one job: the title says the request's kind alone, `Question`, `Plan` or `Permission`, the label row says which one of its kind it is, and the block shows the rest. A part that always says the same kind of thing is read at a glance, and one that repeats another wastes a row: `Request::kind` and `Request::label`, claudio's own rule
- Split a permission's block in two: the call's input in `INPUT_ROWS`, then in `ABOUT_ROWS`, what the request says about itself, why Claude Code asks and who asked, each always there, in grey when Claude Code says nothing, so every part keeps its rows on every request. Who asked goes last, so a short screen hides it first, since it matters least to the answer: `about_fields`, claudio's own rule
- Make each tool's layout fill `INPUT_ROWS` exactly, so a call that gives every field shows them all, and the field that matters most, like a command, or what an edit puts in, takes the most rows: `LAYOUTS`, checked by a test
- Name an MCP server by a field of its own, `mcp_server`, among a call's other fields by name, since only the word MCP tells a person where the tool comes from: `MCP_SERVER`, claudio's own rule

## Keys

- Let the key that names an option, its number or its letter, only move the cursor to it, and Enter answer, so the user sees their choice before it counts, and a slip, like Ctrl+Y that loses its Ctrl, answers nothing. [Primer](https://primer.style/accessibility/patterns/keyboard-shortcuts#avoid-single-character-shortcuts-by-default), [NN/g](https://www.nngroup.com/articles/ui-accelerators/)

## Checks

- Keep the API of the checks a fake: give each kind of request one rule, listed in the doc of `fake_api`, and fail the check on a request that no rule covers. [Software Engineering at Google](https://abseil.io/resources/swe-book/html/ch13.html#faking-id00043)
