/**
 * One note-title algorithm, shared by the sidebar/tab UI and by Rust agent
 * replies. Keep it byte-for-byte equivalent to `note_title()` in
 * `desktop/src-tauri/src/chat.rs`; fixtures in `note-title-fixtures.json` are
 * checked from both sides.
 */

const ESCAPED_MARKDOWN = /\\([\\`*_{}\[\]()#+\-.!])/g;
const LEADING_MARKDOWN = /^[\s#>*+\-`_]+/;
const INLINE_WRAPPERS = /\*\*|__|`/g;
const LIMIT = 52;

export function noteTitle(body: string): string {
  const normalized = body.replace(/\\+/g, '\\').replace(ESCAPED_MARKDOWN, '$1').trim();
  if (!normalized) return 'Untitled note';

  const clean = normalized
    .split('\n')
    .map(line => line.trim().replace(LEADING_MARKDOWN, '').replace(INLINE_WRAPPERS, '').trim())
    .filter(Boolean)
    .join('\n');
  if (!clean) return 'Untitled note';

  const sentences: string[] = [];
  let current = '';
  let index = 0;

  const finish = () => {
    const piece = current.trim();
    if (piece && piece !== sentences[sentences.length - 1]) sentences.push(piece);
    current = '';
  };

  while (index < clean.length) {
    const char = clean[index];
    const punctuation = char === '.' || char === '!' || char === '?';
    // Do not split IPs, versions or decimals: a dot between digits belongs to
    // the word, not to the sentence boundary.
    const insideNumber = char === '.' && /\d/.test(clean[index - 1] ?? '') && /\d/.test(clean[index + 1] ?? '');
    if (char === '\n' || (punctuation && !insideNumber)) {
      if (char === '\n') {
        while (index < clean.length && clean[index] === '\n') {
          current += '\n';
          index += 1;
        }
      } else {
        while (index < clean.length && '.!?'.includes(clean[index])) {
          current += clean[index];
          index += 1;
        }
      }
      finish();
      if (sentences.length === 3) break;
      continue;
    }
    current += char;
    index += 1;
  }
  if (sentences.length < 3) finish();

  const title = sentences.join(' ').replace(/\s+/g, ' ').trim();
  if (!title) return 'Untitled note';
  const chars = Array.from(title);
  if (chars.length > LIMIT) {
    return `${chars.slice(0, LIMIT - 3).join('').trimEnd()}...`;
  }
  return title;
}
