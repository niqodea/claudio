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
