# Notes, chat and sidebar polish — plus safer tab closing

A round of small fixes that make the everyday paths feel calmer: notes behave
better, the AI chat welcome screen is easier to scan, and live sessions no
longer close by accident.

## Notes and AI chat

- Favorites, a clearer search label, and hosts listed above notes in **All** and
  **Favorite**. Note titles are no longer doubled, dates stay anchored to the
  actual device date, and note text is saved as plain Markdown in the vault —
  only credentials stay sealed.
- Code blocks in notes now follow the app theme. Dark mode uses Monaco's dark
  palette, so tokens are readable instead of dark-on-dark.
- The AI chat welcome screen now leads with a large **RuTerm AI Chat** title and
  horizontal suggestion cards. The scrollbar is hidden — swipe with the
  trackpad or use the small `< >` controls — and a gear in the chat header jumps
  straight to **Settings → RuTerm AI Chat (BYOK)**.
- Added a one-click **Get a free API key** link to OpenRouter in the BYOK section.

## Sidebar and home

- The collapsed rail is a little wider, with a steady `>` icon to expand it
  instead of the blinking `R_`. Notes now use a paper icon everywhere.
- Home is flat and minimal again: just the clock and **New host**,
  **Local terminal**, **What's New?**. The hero quick-connect card is gone; the
  compact strip under the tab bar is still there.

## Sessions

- Closing an SSH, quick-connect or SFTP tab now asks **“Close this vault tab?”**
  first. Other tabs still close instantly, and SFTP file viewers close together
  with their parent session.
