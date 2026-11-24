/**
 * Small WYSIWYG Markdown editor helpers.
 *
 * The note editor is a single `contenteditable` surface: markdown syntax is
 * transformed *as it is typed* (`- ` becomes a bullet, `# ` a heading, ...),
 * while the document is kept as plain markdown underneath for storage.
 */
import { loadMonaco } from './components/CodeEditor.svelte';
import { renderSafeMarkdown } from './markdown';
import { languageForLabel } from './languages';
import { ui } from '#lib/ui.svelte';

const BLOCKS = 'p,div,h1,h2,h3,h4,h5,h6,ul,ol,li,blockquote,pre,hr,table';

function escapeText(value: string): string {
  return value.replace(/([\\`*_{}\[\]()#+\-.!])/g, '\\$1');
}

function serializeInline(node: Node): string {
  if (node.nodeType === Node.TEXT_NODE) return escapeText(node.textContent ?? '');
  if (node.nodeType !== Node.ELEMENT_NODE) return '';
  const el = node as HTMLElement;
  const tag = el.tagName.toLowerCase();
  const kids = Array.from(el.childNodes).map(serializeInline).join('');
  switch (tag) {
    case 'br': return '\n';
    case 'strong': case 'b': return `**${kids}**`;
    case 'em': case 'i': return `*${kids}*`;
    case 'del': case 's': case 'strike': return `~~${kids}~~`;
    case 'code': return `\`${kids}\``;
    case 'a': return `[${kids}](${el.getAttribute('href') ?? ''})`;
    case 'img': return `![${el.getAttribute('alt') ?? ''}](${el.getAttribute('src') ?? ''})`;
    default: return kids;
  }
}

/** `textContent` drops `<br>` and block breaks — code needs real newlines. */
function preText(code: HTMLElement): string {
  let out = '';
  for (const node of Array.from(code.childNodes)) {
    if (node.nodeType === Node.TEXT_NODE) {
      out += (node.textContent ?? '').replace(/\u00a0/g, ' ');
    } else if ((node as HTMLElement).tagName === 'BR') {
      out += '\n';
    } else {
      out += preText(node as HTMLElement);
      if ((node as HTMLElement).matches?.('div,p')) out += '\n';
    }
  }
  return out;
}

function serializeBlock(el: HTMLElement): string {
  const tag = el.tagName.toLowerCase();
  if (tag === 'pre') {
    const code = el.querySelector('code');
    const lang = code?.className.match(/language-([\w-]+)/)?.[1] ?? '';
    // The invisible word joiner only keeps the caret on an empty last line.
    return `\`\`\`${lang}\n${code ? preText(code).replace(/\u200b/g, '') : ''}\n\`\`\``;
  }
  if (tag === 'ul' || tag === 'ol') {
    return Array.from(el.children).map((li, i) => {
      const marker = tag === 'ul' ? '-' : `${i + 1}.`;
      return `${marker} ${Array.from(li.childNodes).map(serializeInline).join('')}`;
    }).join('\n');
  }
  if (/^h[1-6]$/.test(tag)) return `${'#'.repeat(Number(tag[1]))} ${Array.from(el.childNodes).map(serializeInline).join('')}`;
  if (tag === 'blockquote') {
    return Array.from(el.childNodes).map(serializeInline).join('')
      .split('\n').map(line => `> ${line}`).join('\n');
  }
  if (tag === 'hr') return '---';
  return Array.from(el.childNodes).map(serializeInline).join('');
}

/** DOM (`contenteditable`) → plain markdown, ready to save into the vault. */
export function serializeEditor(root: HTMLElement): string {
  const lines = Array.from(root.childNodes).map(node =>
    node.nodeType === Node.TEXT_NODE
      ? (node.textContent ?? '')
      : serializeBlock(node as HTMLElement),
  );
  return lines.join('\n').replace(/\n{3,}/g, '\n\n').replace(/^\n+|\n+$/g, '');
}

/** Replace the editor contents with rendered markdown (external note change). */
export function renderEditor(root: HTMLElement, markdown: string): void {
  // Monaco's colorizer emits &nbsp; for indentation; keep code blocks as real
  // spaces so they stay readable and don't encode NBSP back into the vault.
  const clean = (markdown || '').replace(/```([\s\S]*?)```/g, (_all, body: string) =>
    `\`\`\`${body.replace(/\u00a0/g, ' ')}\`\`\``);
  root.innerHTML = renderSafeMarkdown(clean) || '<p><br></p>';
  for (const code of Array.from(root.querySelectorAll('pre > code'))) {
    newlinesToBreaks(code as HTMLElement);
  }
  void colorizeCode(root);
}

function selectionRange(root: HTMLElement): Range | null {
  const selection = window.getSelection();
  if (!selection || selection.rangeCount === 0) return null;
  const range = selection.getRangeAt(0);
  return root.contains(range.startContainer) ? range : null;
}

function closestBlock(node: Node, root: HTMLElement): HTMLElement | null {
  let current: Node | null = node;
  while (current && current !== root) {
    if (current.nodeType === Node.ELEMENT_NODE) {
      const el = current as HTMLElement;
      if (el.matches(BLOCKS)) return el;
    }
    current = current.parentNode;
  }
  return root;
}

function caretIn(node: Node, offset = 0): void {
  const range = document.createRange();
  try { range.setStart(node, offset); } catch { range.setStart(node, 0); }
  range.collapse(true);
  const selection = window.getSelection();
  selection?.removeAllRanges();
  selection?.addRange(range);
}

function caretBeforeNode(node: Node): void {
  const range = document.createRange();
  range.setStartBefore(node);
  range.collapse(true);
  const selection = window.getSelection();
  selection?.removeAllRanges();
  selection?.addRange(range);
}

function caretAfterNode(node: Node): void {
  const range = document.createRange();
  range.setStartAfter(node);
  range.collapse(true);
  const selection = window.getSelection();
  selection?.removeAllRanges();
  selection?.addRange(range);
}

/** Text length of a subtree, counting a line break as one character. */
function subtreeTextLength(node: Node): number {
  if (node.nodeType === Node.TEXT_NODE) return (node as Text).data.length;
  if (node.nodeType === Node.ELEMENT_NODE && (node as HTMLElement).tagName === 'BR') return 1;
  let total = 0;
  for (const child of Array.from(node.childNodes)) total += subtreeTextLength(child);
  return total;
}

/**
 * A fenced block stores its line breaks as `<br>` so the caret can sit on a
 * new line: an insertion at the end of a newline-terminated text node is moved
 * back in front of that newline by Blink/WebKit. Serialization turns every
 * `<br>` back into `\n`, so the saved markdown is unchanged.
 */
function newlinesToBreaks(root: HTMLElement): void {
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  const texts: Text[] = [];
  while (walker.nextNode()) {
    const text = walker.currentNode as Text;
    if (text.data.includes('\n')) texts.push(text);
  }
  for (const text of texts) {
    const fragment = document.createDocumentFragment();
    text.data.split('\n').forEach((line, index) => {
      if (index > 0) fragment.appendChild(document.createElement('br'));
      if (line) fragment.appendChild(document.createTextNode(line));
    });
    text.replaceWith(fragment);
  }
}

/**
 * Monaco emits one `<br/>` per line, including the final line, while a block
 * with N lines has only N-1 separators. Drop the surplus breaks so recolouring
 * can never append blank lines — otherwise every keystroke grows the block.
 */
function trimBreaks(code: HTMLElement, expected: number): void {
  let current = (preText(code).match(/\n/g) ?? []).length;
  while (current > expected) {
    const breaks = code.querySelectorAll('br');
    const last = breaks[breaks.length - 1];
    if (!last) break;
    last.remove();
    current -= 1;
  }
}

/**
 * A block that can actually hold the caret. An empty text node has no layout,
 * so Chrome/WebKit redirect the next keystroke to the previous line — a
 * trailing `<br>` keeps the empty paragraph editable until text arrives.
 */
function emptyBlock(tag: string): HTMLElement {
  const el = document.createElement(tag);
  el.appendChild(document.createElement('br'));
  return el;
}

function swapBlock(root: HTMLElement, old: HTMLElement, next: HTMLElement, caret: Node): void {
  if (old === root) root.appendChild(next);
  else old.replaceWith(next);
  caretIn(caret);
}

/**
 * Live block transformations. Returns true when the input was consumed.
 * Covers the syntax people notice most: lists, headings, quotes and fences.
 */
export function handleBeforeInput(event: InputEvent, root: HTMLElement): boolean {
  const data = event.data;
  const range = selectionRange(root);
  if (!range || range.startContainer.nodeType !== Node.TEXT_NODE) return false;
  const node = range.startContainer as Text;
  if (node.parentElement?.closest('pre, code')) return false;
  const before = (node.textContent ?? '').slice(0, range.startOffset);
  const block = closestBlock(node, root);
  if (!block) return false;

  // ``` + ` → fenced code block.
  if (data === '`' && /^\s*``$/.test(before)) {
    event.preventDefault();
    node.textContent = '';
    const pre = document.createElement('pre');
    const code = document.createElement('code');
    code.className = 'language-plaintext';
    code.appendChild(document.createTextNode(''));
    pre.appendChild(code);
    swapBlock(root, block, pre, code.firstChild!);
    return true;
  }

  if (data !== ' ') return false;
  const match = before.match(/^(\s*)([-*+]|\d+\.|#{1,6}|>)$/);
  if (!match) return false;
  const marker = match[2];
  event.preventDefault();
  node.textContent = (node.textContent ?? '').slice(range.startOffset);

  if (/^[-*+]$/.test(marker) || /^\d+\.$/.test(marker)) {
    const list = document.createElement(/^\d+\.$/.test(marker) ? 'ol' : 'ul');
    const li = document.createElement('li');
    li.appendChild(document.createElement('br'));
    list.appendChild(li);
    swapBlock(root, block, list, li);
    return true;
  }
  if (/^#{1,6}$/.test(marker)) {
    const heading = emptyBlock(`h${marker.length}`);
    swapBlock(root, block, heading, heading);
    return true;
  }
  if (marker === '>') {
    const quote = document.createElement('blockquote');
    const para = emptyBlock('p');
    quote.appendChild(para);
    swapBlock(root, block, quote, para);
    return true;
  }
  return false;
}

/** Turn `**bold**`, `*italic*` and `` `code` `` into elements while typing. */
export function formatInline(root: HTMLElement): void {
  const range = selectionRange(root);
  if (!range || range.startContainer.nodeType !== Node.TEXT_NODE) return;
  if ((range.startContainer as Text).parentElement?.closest('pre, code')) return;
  const block = closestBlock(range.startContainer, root);
  if (!block) return;

  const walker = document.createTreeWalker(block, NodeFilter.SHOW_TEXT);
  const texts: Text[] = [];
  while (walker.nextNode()) texts.push(walker.currentNode as Text);

  for (const text of texts) {
    const value = text.textContent ?? '';
    const pattern = /(\*\*([^*\n]+)\*\*|\*([^*\n]+)\*|`([^`\n]+)`)/;
    const match = value.match(pattern);
    if (!match || match.index === undefined) continue;
    const token = match[1];
    const tag = token.startsWith('**') ? 'strong'
      : token.startsWith('*') ? 'em' : 'code';
    const inner = token.startsWith('**') ? match[2]
      : token.startsWith('*') ? match[3] : match[4];

    const element = document.createElement(tag);
    element.textContent = inner;
    const fragment = document.createDocumentFragment();
    const before = value.slice(0, match.index);
    const after = value.slice(match.index + token.length);
    if (before) fragment.appendChild(document.createTextNode(before));
    fragment.appendChild(element);
    let caret: Node = element;
    if (after) {
      const tail = document.createTextNode(after);
      fragment.appendChild(tail);
      caret = tail;
    }
    text.replaceWith(fragment);
    caretIn(caret, after ? 0 : 1);
    return; // one transform per keystroke keeps the caret predictable
  }
}

/** Enter leaves a code block; Shift+Enter is a newline inside it. */
export function handleEnter(event: KeyboardEvent, root: HTMLElement): boolean {
  if (event.key !== 'Enter') return false;
  const range = selectionRange(root);
  if (!range) return false;
  const node = range.startContainer;
  // Monaco colouring wraps code in spans, so the caret can sit on an element
  // instead of a text node. Handle both rather than falling back to the browser.
  const host = node.nodeType === Node.TEXT_NODE
    ? (node as Text).parentElement
    : node as HTMLElement;

  if (host?.closest('pre')) {
    event.preventDefault();
    // Plain Enter returns to normal text; Shift+Enter stays in the code block.
    if (!event.shiftKey) return exitCodeBlock(root);
    // Shift+Enter inserts a real line break element, then parks the caret
    // after it. The next keystroke becomes a text node on the new line instead
    // of being swallowed in front of a trailing "\n" character. When the
    // break lands at the end, an invisible word joiner gives the caret a
    // non-empty node to sit in; it is stripped when the note is serialized.
    const code = host.closest('code') as HTMLElement | null;
    const br = document.createElement('br');
    range.deleteContents();
    range.insertNode(br);
    if (code) delete code.dataset.colored;
    const next = br.nextSibling;
    if (next && (next.nodeType !== Node.TEXT_NODE || (next as Text).data.length > 0)) {
      caretBeforeNode(next);
    } else {
      const anchor = document.createTextNode('\u200b');
      br.after(anchor);
      caretBeforeNode(anchor);
    }
    return true;
  }

  // Outside code, Shift+Enter keeps the browser's default soft line break.
  if (event.shiftKey) return false;
  if (node.nodeType !== Node.TEXT_NODE) return false;
  const textNode = node as Text;

  const li = textNode.parentElement?.closest('li');
  if (!li || !root.contains(li) || (li.textContent ?? '').trim() !== '') return false;

  event.preventDefault();
  const list = li.parentElement;
  const para = emptyBlock('p');
  if (list && list.parentElement) list.after(para);
  else root.appendChild(para);
  if (!(li.textContent ?? '').length && li.parentElement?.children.length === 1) li.remove();
  caretIn(para, 0);
  return true;
}

const CODE_KEYWORD = /^\s*(?:function|const|let|var|class|def|import|from|package|using|include|public|private|static|async|return|if|for|while|switch|try|catch|SELECT|INSERT|UPDATE|DELETE|CREATE|ALTER|DROP)\b/m;

function looksLikeCode(text: string): boolean {
  const lines = text.split(/\r?\n/).filter(line => line.trim().length > 0);
  if (!lines.length) return false;
  let score = 0;
  for (const line of lines.slice(0, 40)) {
    if (/[{};]\s*$/.test(line)) score += 2;
    if (/=>|==|!=|<=|>=|&&|\|\||::/.test(line)) score += 2;
    if (CODE_KEYWORD.test(line)) score += 2;
    if (/^\s{2,}\S|^\t\S/.test(line)) score += 1;
    if (/["`']/.test(line) && /[()[\]{},;:]/.test(line)) score += 1;
  }
  // Markdown prose (lists, quotes) stays prose even when it has punctuation.
  const prose = lines.filter(line => /^\s*(?:[-*+]\s|>\s|\d+\.\s)/.test(line)).length;
  if (prose > lines.length / 2) return false;
  return score >= (lines.length > 1 ? 3 : 4);
}

function guessLanguage(text: string): string {
  if (/<\?php/.test(text)) return 'php';
  if (/<!doctype|<html|<div|<span/i.test(text)) return 'html';
  if (/\bselect\b[\s\S]*\bfrom\b/i.test(text)) return 'sql';
  if (/^\s*(?:def\s+\w+\s*\(|print\s*\()/m.test(text)) return 'python';
  if (/\bfn\s+main\s*\(|println!|let\s+mut\s/.test(text)) return 'rust';
  if (/^\s*func\s+\w+\s*\(/m.test(text)) return 'go';
  if (/public\s+class|System\.out/.test(text)) return 'java';
  if (/=>|console\.log|\bfunction\b|\bconst\s|\blet\s/.test(text)) return 'javascript';
  if (/^#!|\b(?:sudo|apt|apt-get|yum|dnf|npm|yarn|pnpm|git|kubectl|docker|systemctl|echo|curl|wget|ssh)\b/.test(text)) return 'bash';
  return 'plaintext';
}

function hasHtmlTagPair(text: string): boolean {
  const tags = Array.from(text.matchAll(/<\/?([a-z][\w-]*)\b[^>]*>/gi)).map(match => ({
    name: match[1].toLowerCase(),
    closing: match[0].startsWith('</'),
  }));
  return tags.some(tag => !tag.closing
    && tags.some(other => other.closing && other.name === tag.name));
}

const STRONG_CODE = [
  /=>/,
  /\bfunction\s+\w+\s*\(/,
  /\b(?:const|let|var)\s+\w+\s*=/,
  /\bimport\s+.+\s+from\s+/,
  /^\s*def\s+\w+\s*\(/m,
  /^#!/,
  /\bselect\b[\s\S]*\bfrom\b/i,
  /[{}]\s*;/,
];

/** Recognise code in pasted or typed text, including a single HTML tag pair. */
function codeFromText(text: string): { code: string; language: string } | null {
  const fenced = text.trim().match(/^```([\w+-]*)\s*\n([\s\S]*?)\n?```\s*$/);
  if (fenced) {
    return {
      language: fenced[1] || 'plaintext',
      code: fenced[2].replace(/\s+$/, ''),
    };
  }
  const value = text.replace(/^\n+|\n+$/g, '');
  if (hasHtmlTagPair(value)) return { code: value, language: 'html' };
  if (!looksLikeCode(value) && !STRONG_CODE.some(pattern => pattern.test(value))) return null;
  return { code: value, language: guessLanguage(value) };
}

/**
 * Paste code straight into a fenced block. Plain prose keeps the browser's
 * normal paste behaviour; only recognisable code (or an explicit ``` fence)
 * is transformed. Returns true when the paste was consumed.
 */
export function handlePaste(event: ClipboardEvent, root: HTMLElement): boolean {
  const raw = event.clipboardData?.getData('text/plain');
  if (!raw) return false;
  const payload = codeFromText(raw.replace(/\r\n?/g, '\n'));
  if (!payload) return false;

  const range = selectionRange(root);
  if (!range) return false;
  if ((range.startContainer.nodeType === Node.TEXT_NODE
      ? range.startContainer.parentElement
      : range.startContainer as HTMLElement)?.closest('pre, code')) return false;

  event.preventDefault();
  const pre = document.createElement('pre');
  const code = document.createElement('code');
  code.className = `language-${payload.language}`;
  code.appendChild(document.createTextNode(payload.code));
  newlinesToBreaks(code);
  pre.appendChild(code);

  const block = closestBlock(range.startContainer, root);
  if (block && block !== root && !(block.textContent ?? '').trim()) block.replaceWith(pre);
  else if (block && block !== root) block.after(pre);
  else root.appendChild(pre);
  if (pre.nextSibling === null) {
    const tail = emptyBlock('p');
    pre.after(tail);
  }
  setCaretTextOffset(code, payload.code.length);
  return true;
}

/**
 * Typing a complete code token (`<p>…</p>`, `;`, `}`, ...) turns the paragraph
 * into a fenced block immediately, so code never stays as loose prose.
 */
export function detectTypedCode(event: Event, root: HTMLElement): boolean {
  void event;
  const range = selectionRange(root);
  if (!range || range.startContainer.nodeType !== Node.TEXT_NODE) return false;
  const node = range.startContainer as Text;
  if (node.parentElement?.closest('pre, code')) return false;
  const block = closestBlock(node, root);
  // An emptied editor can hold bare text directly under the root. Treat that
  // as one virtual block and don't rewrite the DOM on every keystroke.
  const atRoot = block === root;
  if (!atRoot && (!block || !/^(p|div)$/i.test(block.tagName))) return false;
  if (!atRoot && block!.querySelector('ul,ol,blockquote,h1,h2,h3,h4,h5,h6,table,pre')) return false;

  const payload = codeFromText((atRoot ? root.textContent : block!.textContent ?? '').trim());
  if (!payload) return false;

  // Only replace the root when it holds plain text; otherwise another block
  // (e.g. a note already containing paragraphs) must not be destroyed.
  if (atRoot && Array.from(root.childNodes).some(child =>
    child.nodeType === Node.ELEMENT_NODE && (child as HTMLElement).tagName !== 'BR')) {
    return false;
  }

  const pre = document.createElement('pre');
  const code = document.createElement('code');
  code.className = `language-${payload.language}`;
  const text = document.createTextNode(payload.code);
  code.appendChild(text);
  pre.appendChild(code);
  if (atRoot) root.replaceChildren(pre, emptyBlock('p'));
  else {
    block!.replaceWith(pre);
    if (pre.nextSibling === null) pre.after(emptyBlock('p'));
  }
  caretIn(text, text.length);
  root.focus();
  return true;
}

/** Move the caret out of a fenced block into a normal paragraph. */
export function exitCodeBlock(root: HTMLElement): boolean {
  const range = selectionRange(root);
  if (!range) return false;
  const node = range.startContainer;
  const host = node.nodeType === Node.TEXT_NODE
    ? node.parentElement
    : node as HTMLElement;
  const pre = host?.closest('pre');
  if (!pre || !root.contains(pre)) return false;

  let para = pre.nextElementSibling as HTMLElement | null;
  if (!para || !/^(p|div)$/i.test(para.tagName) || para.querySelector('pre,ul,ol,blockquote')) {
    para = emptyBlock('p');
    pre.after(para);
  }
  // Target the paragraph itself: the caret belongs before its `<br>`, so the
  // next keystroke lands here instead of falling back into the code block.
  caretIn(para, 0);
  root.focus();
  return true;
}

/** Character offset of the caret inside a fenced block, counting `<br>` as one. */
function caretTextOffset(code: HTMLElement): number | null {
  const selection = window.getSelection();
  if (!selection?.rangeCount) return null;
  const range = selection.getRangeAt(0);
  const start = range.startContainer;
  if (!code.contains(start)) return null;

  const children = start === code ? Array.from(code.childNodes) : [];
  if (start === code) {
    let inside = 0;
    for (let i = 0; i < Math.min(range.startOffset, children.length); i++) {
      inside += subtreeTextLength(children[i]);
    }
    return inside;
  }

  let total = 0;
  const walker = document.createTreeWalker(code, NodeFilter.SHOW_TEXT | NodeFilter.SHOW_ELEMENT);
  let node = walker.nextNode();
  while (node) {
    if (node === start) {
      if (node.nodeType === Node.TEXT_NODE) return total + range.startOffset;
      // Element container (e.g. a Monaco span): sum the children before it.
      const kids = Array.from((node as HTMLElement).childNodes);
      let inside = 0;
      for (let i = 0; i < Math.min(range.startOffset, kids.length); i++) {
        inside += subtreeTextLength(kids[i]);
      }
      return total + inside;
    }
    if (node.nodeType === Node.TEXT_NODE) total += (node as Text).data.length;
    else if ((node as HTMLElement).tagName === 'BR') total += 1;
    node = walker.nextNode();
  }
  return total;
}

function setCaretTextOffset(code: HTMLElement, offset: number): void {
  let remaining = offset;
  const walker = document.createTreeWalker(code, NodeFilter.SHOW_TEXT | NodeFilter.SHOW_ELEMENT);
  let node = walker.nextNode();
  while (node) {
    if (node.nodeType === Node.TEXT_NODE) {
      const length = (node as Text).data.length;
      if (remaining < length) {
        caretIn(node, remaining);
        return;
      }
      if (remaining === length) {
        caretIn(node, length);
        return;
      }
      remaining -= length;
    } else if ((node as HTMLElement).tagName === 'BR') {
      if (remaining === 0) {
        caretBeforeNode(node);
        return;
      }
      remaining -= 1;
      if (remaining === 0) {
        caretAfterNode(node);
        return;
      }
    }
    node = walker.nextNode();
  }
  const last = code.lastChild;
  if (last) caretAfterNode(last);
  else caretIn(code, 0);
}

let colorizeSequence = 0;
let colorizeTheme: string | null = null;

/** Monaco token colours for fenced code blocks inside the live editor. */
export async function colorizeCode(root: HTMLElement): Promise<void> {
  const blocks = Array.from(root.querySelectorAll('pre > code')) as HTMLElement[];
  if (!blocks.length) return;
  const sequence = ++colorizeSequence;
  const theme = ui.theme === 'light' ? 'rterm-light' : 'rterm-dark';
  const themeChanged = colorizeTheme !== theme;
  colorizeTheme = theme;
  try {
    const monaco = await loadMonaco();
    if (sequence !== colorizeSequence || !root.isConnected) return;
    // colorize() bakes the active theme into inline styles. Without this the
    // light default leaks dark tokens into dark-theme notes.
    monaco.editor.setTheme(theme);
    for (const code of blocks) {
      const offset = caretTextOffset(code);
      const source = preText(code);
      if (!source || (!themeChanged && code.dataset.colored === source)) continue;
      const label = code.className.match(/language-([\w-]+)/)?.[1] ?? 'plaintext';
      const html = await monaco.editor.colorize(source, languageForLabel(label), { tabSize: 2 });
      if (sequence !== colorizeSequence || !code.isConnected) return;
      code.innerHTML = html;
      newlinesToBreaks(code);
      trimBreaks(code, (source.match(/\n/g) ?? []).length);
      // Compare against the normalised text, so an already-coloured block is
      // skipped instead of being re-coloured on every keystroke.
      code.dataset.colored = preText(code);
      if (offset !== null) setCaretTextOffset(code, offset);
    }
  } catch {
    // Colouring is optional; plain text is always valid inside the editor.
  }
}
