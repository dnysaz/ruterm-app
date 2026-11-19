// Open links in the OS default browser/app instead of inside the Tauri webview.

import { openUrl } from '@tauri-apps/plugin-opener';

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

export async function openExternalLink(rawHref: string | null | undefined) {
  const href = toExternalHref(rawHref);
  if (!href) return;
  try {
    await openUrl(href);
  } catch {
    // Rare fallback (e.g. running in a plain browser preview).
    window.open(href, '_blank', 'noopener,noreferrer');
  }
}
