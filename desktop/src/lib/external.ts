// Open links in the OS default browser/app instead of inside the Tauri webview.

import { invoke } from '@tauri-apps/api/core';

const EXTERNAL = /^(https?|mailto|tel):/i;

/// Normalize a clicked href or return null when it's not an external link.
export function toExternalHref(raw: string | null | undefined): string | null {
  if (!raw) return null;
  const href = raw.trim();
  if (!href || href.startsWith('#') || href.toLowerCase().startsWith('javascript:')) {
    return null;
  }
  if (EXTERNAL.test(href)) return href;
  if (/^(www\.|[\w-]+(\.[\w-]+)+)(\/|$|\?)/.test(href)) return `https://${href}`;
  return null;
}

export function isExternalLink(rawHref: string | null | undefined): boolean {
  return toExternalHref(rawHref) !== null;
}

/// Anchors with a rejected scheme must never keep their native behaviour —
/// otherwise `javascript:` / `data:` hrefs still execute (F-51).
export function isUnsafeHref(rawHref: string | null | undefined): boolean {
  if (!rawHref) return false;
  const href = rawHref.trim();
  if (!href || href.startsWith('#')) return false;
  // Protocol-relative URLs (`//evil.example`) navigate off-app and must be
  // blocked just like any other untrusted target (V-04).
  if (href.startsWith('//')) return true;
  if (href.startsWith('/') || href.startsWith('./') || href.startsWith('../')) {
    return false;
  }
  return toExternalHref(href) === null;
}

export async function openExternalLink(rawHref: string | null | undefined) {
  const href = toExternalHref(rawHref);
  if (!href) return;
  try {
    // Rust re-validates the scheme/authority; the renderer cannot reach the
    // opener plugin directly (F-32).
    await invoke('open_external_link', { url: href });
  } catch {
    // Plain-browser preview (no Tauri IPC). Re-apply the strict Rust-side
    // rules here so a URL rejected by Rust cannot be re-opened via fallback
    // (F-32): no credentials/@, no backslash, no control chars/whitespace.
    if (/[\s\\@\u0000-\u001f\u007f]/u.test(href)) return;
    if (!EXTERNAL.test(href)) return;
    window.open(href, '_blank', 'noopener,noreferrer');
  }
}
