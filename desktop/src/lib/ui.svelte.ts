import { api } from '$lib/api';

/**
 * App-wide UI scale. Tauri scales the whole webview natively, so xterm and the
 * Svelte UI stay pixel-crisp instead of being CSS-transformed.
 */
const STORAGE_KEY = 'rterm.zoom';
const MIN = 0.8;
const MAX = 1.6;
const STEP = 0.1;

function read(): number {
  try {
    const raw = Number(localStorage.getItem(STORAGE_KEY));
    if (Number.isFinite(raw) && raw >= MIN && raw <= MAX) return Math.round(raw * 10) / 10;
  } catch {
    /* private mode */
  }
  return 1;
}

export type Theme = 'ember-black' | 'ember-light' | 'pine-black';

export const THEMES: { id: Theme; label: string; hint: string }[] = [
  { id: 'ember-black', label: 'Ember Black', hint: 'Orange on black' },
  { id: 'ember-light', label: 'Ember Light', hint: 'Orange on ivory' },
  { id: 'pine-black', label: 'Pine Black', hint: 'Green on black' },
];

const THEME_KEY = 'rterm.theme';
const LEGACY: Record<string, Theme> = { dark: 'ember-black', light: 'ember-light' };

function readTheme(): Theme {
  try {
    const stored = localStorage.getItem(THEME_KEY);
    if (stored && stored in LEGACY) return LEGACY[stored];
    if (stored === 'ember-black' || stored === 'ember-light' || stored === 'pine-black') {
      return stored;
    }
  } catch {
    /* ignore */
  }
  return 'ember-black';
}

export const ui = $state({ zoom: read(), theme: readTheme() });

export function applyTheme(theme: Theme) {
  ui.theme = theme;
  document.documentElement.dataset.theme = theme;
  try {
    localStorage.setItem(THEME_KEY, theme);
  } catch {
    /* ignore */
  }
  void api.uiSetTheme(theme).catch(() => {});
}

function round(value: number) {
  return Math.min(MAX, Math.max(MIN, Math.round(value * 10) / 10));
}

export async function applyZoom(next: number) {
  const scale = round(next);
  if (scale === ui.zoom) return;
  ui.zoom = scale;
  try {
    localStorage.setItem(STORAGE_KEY, String(scale));
  } catch {
    /* ignore */
  }
  await api.uiSetZoom(scale).catch(() => {});
}

export function zoomIn() {
  void applyZoom(ui.zoom + STEP);
}

export function zoomOut() {
  void applyZoom(ui.zoom - STEP);
}

export function zoomReset() {
  void applyZoom(1);
}

/** Re-apply the saved scale after the webview reloads. */
export function restoreZoom() {
  void api.uiSetZoom(ui.zoom).catch(() => {});
}

/** Paint the persisted theme before the first frame. */
export function restoreTheme() {
  document.documentElement.dataset.theme = ui.theme;
  void api.uiSetTheme(ui.theme).catch(() => {});
}
