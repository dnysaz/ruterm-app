/**
 * Shared Markdown renderer for the two `{@html}` sinks in the app.
 *
 * Remote release notes and user notes are untrusted input. We use a small
 * allowlist (no forms, inputs, SVG, style, frames) and hook attribute
 * sanitization so a link can never point at `javascript:` or a local file.
 */
import { marked } from 'marked';
import DOMPurify from 'dompurify';

const ALLOWED_TAGS = [
  'p', 'br', 'hr', 'h1', 'h2', 'h3', 'h4', 'h5', 'h6',
  'ul', 'ol', 'li', 'blockquote', 'pre', 'code',
  'strong', 'em', 'del', 's', 'u', 'sub', 'sup',
  'a', 'img', 'table', 'thead', 'tbody', 'tr', 'th', 'td',
];

const ALLOWED_ATTR = ['href', 'title', 'alt', 'src', 'width', 'height', 'align', 'class', 'id'];

let hookInstalled = false;

function installHook(): void {
  if (hookInstalled || typeof DOMPurify?.addHook !== 'function') return;
  hookInstalled = true;
  DOMPurify.addHook('afterSanitizeAttributes', (node: Element) => {
    const href = node.getAttribute('href');
    if (href !== null) {
      if (/^(https?:\/\/|mailto:|tel:|#)/i.test(href.trim())) {
        if (node.tagName === 'A') {
          node.setAttribute('target', '_blank');
          node.setAttribute('rel', 'noopener noreferrer');
        }
      } else {
        node.removeAttribute('href');
      }
    }
    const src = node.getAttribute('src');
    if (src !== null) {
      const value = src.trim();
      if (!/^(https:\/\/|data:image\/(png|jpeg|gif|webp);base64,)/i.test(value)) {
        node.removeAttribute('src');
      }
    }
  });
}

/** Returns sanitized HTML, or escaped source if the sanitizer is unavailable. */
export function renderSafeMarkdown(
  markdown: string,
  options: { linkBase?: string; imageBase?: string } = {},
): string {
  let source = markdown ?? '';
  if (options.linkBase) {
    source = source.replace(/(\]\()(?!https?:\/\/|#|mailto:)([^)]+)(\))/g, `$1${options.linkBase}$2$3`);
  }
  if (options.imageBase) {
    source = source.replace(/!\[([^\]]*)\]\((?!https?:\/\/|data:)([^)]+)\)/g,
      `![$1](${options.imageBase}$2)`);
  }
  let html: string;
  try {
    html = marked.parse(source, { gfm: true, breaks: true }) as string;
  } catch {
    return escapeHtml(markdown ?? '');
  }
  if (typeof DOMPurify?.sanitize !== 'function') return escapeHtml(html);
  installHook();
  // No `USE_PROFILES` here: in DOMPurify 3.x it overrides ALLOWED_TAGS/ATTR
  // with the full HTML profile and widens the allowlist. The explicit narrow
  // lists above are the intended policy.
  return DOMPurify.sanitize(html, {
    ALLOWED_TAGS,
    ALLOWED_ATTR,
    FORBID_TAGS: ['form', 'input', 'button', 'select', 'textarea', 'label', 'fieldset',
      'style', 'iframe', 'object', 'embed', 'svg', 'math', 'audio', 'video', 'link', 'meta'],
    FORBID_ATTR: ['style', 'srcset', 'formaction', 'onerror', 'onload'],
  });
}

function escapeHtml(value: string): string {
  return value.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

export type ChatSegment =
  | { kind: 'text'; text: string }
  | { kind: 'code'; text: string; label: string };

const FENCED_CODE = /```([^\n`]*)\n([\s\S]*?)```/g;

function looksLikeShell(code: string): boolean {
  return /^\s*(?:sudo|apt(?:-get)?|yum|dnf|brew|cd|ls|cat|echo|curl|wget|ssh|scp|rsync|systemctl|service|nginx|docker|git|mkdir|chmod|chown|python3?|node|npm|pnpm|yarn|tail|grep|awk|sed|df|du|top|htop)\b/m.test(
    code,
  );
}

/**
 * Split a chat reply into prose and fenced code blocks. Rendering code as its
 * own card (instead of inside the markdown HTML) gives every block a language
 * label and a dedicated Copy button.
 */
export function parseChatSegments(markdown: string): ChatSegment[] {
  const source = markdown ?? '';
  const segments: ChatSegment[] = [];
  let cursor = 0;
  FENCED_CODE.lastIndex = 0;
  let match: RegExpExecArray | null;
  while ((match = FENCED_CODE.exec(source))) {
    const prose = source.slice(cursor, match.index).trim();
    if (prose) segments.push({ kind: 'text', text: prose });
    const language = match[1].trim().toLowerCase();
    const code = match[2].replace(/\n$/, '');
    segments.push({
      kind: 'code',
      text: code,
      label: language || (looksLikeShell(code) ? 'bash' : 'code'),
    });
    cursor = FENCED_CODE.lastIndex;
  }
  const tail = source.slice(cursor).trim();
  if (tail) segments.push({ kind: 'text', text: tail });
  return segments;
}
