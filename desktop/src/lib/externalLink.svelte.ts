// Shared state for the "Open external link?" confirmation, plus the delegated
// click handler that routes every external <a> through the modal.

import { toExternalHref } from './external';

export const externalLink = $state({ href: null as string | null });

export function closeExternalLink() {
  externalLink.href = null;
}

/// True when the clicked href is external and the modal was shown.
export function requestExternalLink(raw: string | null): boolean {
  const href = toExternalHref(raw);
  if (!href) return false;
  externalLink.href = href;
  return true;
}

export function installExternalLinkHandler() {
  if (typeof document === 'undefined') return () => {};

  const handler = (event: MouseEvent) => {
    const target = event.target;
    if (!(target instanceof Element)) return;
    const anchor = target.closest('a');
    if (!anchor) return;
    const href = anchor.getAttribute('href');
    if (!requestExternalLink(href)) return;
    event.preventDefault();
    event.stopPropagation();
  };

  document.addEventListener('click', handler, true);
  return () => document.removeEventListener('click', handler, true);
}
