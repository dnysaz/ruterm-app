import { zoomIn, zoomOut, zoomReset } from '#lib/ui.svelte';

const isMac = navigator.platform.toLowerCase().includes('mac');

function emit(name: string) {
  window.dispatchEvent(new CustomEvent(name));
}

/**
 * Desktop shortcuts. Anything handled here is swallowed before xterm or the
 * webview sees it, so the same combos behave the same in every tab.
 */
export function installShortcuts(): () => void {
  function onKeyDown(event: KeyboardEvent) {
    const mod = isMac ? event.metaKey : event.ctrlKey;

    // Ctrl+Tab / Ctrl+Shift+Tab walk the session tabs (works over xterm too).
    if (event.ctrlKey && !event.metaKey && !event.altKey && event.key === 'Tab') {
      event.preventDefault();
      emit(event.shiftKey ? 'rterm:prev-tab' : 'rterm:next-tab');
      return;
    }

    if (!mod || event.altKey) return;

    const key = event.key.toLowerCase();

    switch (key) {
      case '=':
      case '+':
        event.preventDefault();
        zoomIn();
        return;
      case '-':
        event.preventDefault();
        zoomOut();
        return;
      case '0':
        event.preventDefault();
        zoomReset();
        return;
      case 'r':
        event.preventDefault();
        window.location.reload();
        return;
      case 't':
        event.preventDefault();
        emit('rterm:new-session');
        return;
      case 'w':
        event.preventDefault();
        emit('rterm:close-tab');
        return;
      default:
        // Copy / paste / cut / select-all are handled natively by the webview,
        // including inside xterm's textarea — leave them alone.
        return;
    }
  }

  window.addEventListener('keydown', onKeyDown, true);
  return () => window.removeEventListener('keydown', onKeyDown, true);
}
