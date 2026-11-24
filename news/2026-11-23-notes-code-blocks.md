# Notes: type code, get a code block — Enter gets you out again

Notes now recognise code as you type it. A line like `<p> hello </p>` becomes a
fenced code block with the right language attached, instead of staying as loose
prose.

- **Shift+Enter** adds a new line *inside* the code block, so multi-line code
  stays together in one block.
- **Enter** leaves the block and drops you into a normal paragraph, ready for
  prose again.
- Pasting a recognised snippet does the same, and a block never becomes a trap:
  whatever you type after it stays in your text.

This release also fixes a silly one: colouring a block with Monaco added one
extra line break every time it ran, so a single Shift+Enter could leave a
growing stack of blank lines. Colour is now length-stable — one Shift+Enter is
exactly one new line, no matter how long you keep typing.
