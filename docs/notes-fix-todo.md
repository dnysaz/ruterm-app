# Notes follow-ups — fix checklist

Status of the five findings in `docs/notes-followups.md`.

- [x] Validate all five findings against current code and the live vault.
- [x] Unify the note-title algorithm (UI + Rust agent replies) and test it with shared fixtures.
- [x] Add a favorite toggle to note rows and persist it with `saveNote`.
- [x] Show favorited notes in the Favorite view, and show notes together with hosts in All.
- [x] Make search placeholder and `aria-label` follow the active filter.
- [x] Correct the "encrypted notes" copy in News, README, chat/MCP tool descriptions and replies.
- [x] Regenerate `News/index.json` after the News copy change.
- [x] Verify: Rust tests, Svelte check, shared title fixtures, diff review.
- [ ] Optional after explicit approval: rebuild release and reinstall to `/Applications`.

## Verification

- `cargo test --all-targets`: 42 passed.
- `npm run check`: 0 errors, 0 warnings.
- `npm run check:titles`: 7/7 shared fixtures passed.
- `News/index.json` regenerated from the corrected News copy.

## Where each fix landed

1. Favorites: `noteRow` snippet with `★/☆` toggle in `VaultShell.svelte`, persisted via `saveNote`.
2. Search: `searchLabel` derived from the active filter, used for both placeholder and `aria-label`.
3. Titles: `desktop/src/lib/note-title.ts` and `note_title()` in `chat.rs`, checked against the same `note-title-fixtures.json`.
4. Copy: News, README, chat/MCP descriptions and replies, core model comments, plus regression tests against "encrypted vault" phrasing.
5. All/Favorite: both render notes and hosts in separate labelled groups; All is now literally all saved items.

No commit or push without separate explicit approval.
