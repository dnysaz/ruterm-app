/**
 * Reactive background state for the app shell, persisted in
 * localStorage. Exports `background` (read it in markup) and `backgroundStyle`
 * (inline style string for the target element — the empty-main panel).
 */
import {
  BACKGROUND_IMAGES,
  BACKGROUND_PATTERNS,
  DEFAULT_BACKDROP_IMAGE,
  DEFAULT_PATTERN,
  findImage,
  findPattern,
  type BackgroundMode,
} from './background';

const STORAGE_KEY = 'rterm.background.v1';
/** The user's own photo (data URL from disk, or a pasted link). Kept separate
    from the main blob so the tiny preferences JSON stays small. */
const CUSTOM_KEY = 'rterm.background.custom.v1';

export interface BackgroundChoice {
  mode: BackgroundMode;
  imageId: string;
  patternId: string;
  /** 0–85: frost strength for the text card — drives blur radius and tint. */
  dim: number;
  /** 0–85: how much of the backdrop shows through the terminal area. */
  termDim: number;
}

const DEFAULTS: BackgroundChoice = {
  mode: 'none',
  imageId: DEFAULT_BACKDROP_IMAGE,
  patternId: DEFAULT_PATTERN,
  dim: 60,
  termDim: 0,
};

const clampDim = (value: unknown, fallback = DEFAULTS.dim): number => {
  const n = Number(value);
  if (!Number.isFinite(n)) return fallback;
  return Math.min(85, Math.max(0, Math.round(n)));
};

// F-54: a custom wallpaper must be a base64 data URL of a supported image
// type. Anything else (CSS injection, remote tracking URLs, script tricks)
// is rejected both when persisted and when read back from localStorage.
const MAX_CUSTOM_BYTES = 8 * 1024 * 1024;
const DATA_IMAGE_RE = /^data:image\/(?:png|jpeg|gif|webp);base64,[A-Za-z0-9+/]+={0,2}$/;

function isValidDataImage(value: unknown): value is string {
  return typeof value === 'string'
    && value.length <= MAX_CUSTOM_BYTES
    && DATA_IMAGE_RE.test(value);
}

/** Escapes a value for use inside `url("…")` in an inline style. */
function cssUrl(value: string): string {
  return value.replace(/[\\")]/g, c => `\\${c}`).replace(/\n|\r/g, '');
}

function read(): BackgroundChoice {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return { ...DEFAULTS };
    const parsed = JSON.parse(raw) as Partial<BackgroundChoice> | null;
    const mode =
      parsed?.mode === 'image' || parsed?.mode === 'pattern' || parsed?.mode === 'none'
        ? parsed.mode
        : DEFAULTS.mode;
    return {
      mode,
      imageId:
        parsed?.imageId === 'custom' && readCustom()
          ? 'custom'
          : BACKGROUND_IMAGES.find(i => i.id === parsed?.imageId)?.id ?? DEFAULTS.imageId,
      patternId:
        BACKGROUND_PATTERNS.find(p => p.id === parsed?.patternId)?.id ?? DEFAULTS.patternId,
      dim: clampDim(parsed?.dim),
      termDim: clampDim(parsed?.termDim, DEFAULTS.termDim),
    };
  } catch {
    return { ...DEFAULTS };
  }
}

function readCustom(): string | null {
  try {
    const value = localStorage.getItem(CUSTOM_KEY);
    return isValidDataImage(value) ? value : null;
  } catch {
    return null;
  }
}

export const background = $state<BackgroundChoice>(read());

/** The user's own wallpaper (data URL from disk, or a pasted link). */
export const backgroundCustom = $state<{ src: string | null }>({ src: readCustom() });

function persist() {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(background));
  } catch {
    /* private mode — background just doesn't persist */
  }
}

export function setBackgroundMode(mode: BackgroundMode) {
  background.mode = mode;
  persist();
}

export function setBackgroundImage(imageId: string) {
  if (!BACKGROUND_IMAGES.some(i => i.id === imageId)) return;
  background.mode = 'image';
  background.imageId = imageId;
  persist();
}

export function setBackgroundPattern(patternId: string) {
  if (!BACKGROUND_PATTERNS.some(p => p.id === patternId)) return;
  background.mode = 'pattern';
  background.patternId = patternId;
  persist();
}

/**
 * Store the user's wallpaper and select it. Returns false when it could not
 * be persisted (e.g. storage quota) — the image still applies for this
 * session, just not across restarts.
 */
export function setBackgroundCustom(src: string): boolean {
  if (!isValidDataImage(src)) return false;
  backgroundCustom.src = src;
  let saved = true;
  try {
    localStorage.setItem(CUSTOM_KEY, src);
  } catch {
    saved = false;
  }
  background.mode = 'image';
  background.imageId = 'custom';
  persist();
  return saved;
}

/** Re-select the stored custom wallpaper without re-uploading it. */
export function selectBackgroundCustom() {
  if (!backgroundCustom.src) return;
  background.mode = 'image';
  background.imageId = 'custom';
  persist();
}

export function clearBackgroundCustom() {
  backgroundCustom.src = null;
  try {
    localStorage.removeItem(CUSTOM_KEY);
  } catch {
    /* ignore */
  }
  if (background.imageId === 'custom') {
    background.imageId = DEFAULTS.imageId;
    persist();
  }
}

export function setBackgroundDim(value: number) {
  background.dim = clampDim(value);
  persist();
}

export function setBackgroundTermDim(value: number) {
  background.termDim = clampDim(value, DEFAULTS.termDim);
  persist();
}

/**
 * Inline style for the shell — one photo/pattern spanning the whole window,
 * continuing behind the translucent sidebar. `none` returns an empty string
 * so the panel keeps its plain var(--panel) background. Images are rendered
 * sharp — the dim slider drives the frosted card behind the text instead.
 */
export function backgroundStyle(): string {
  if (background.mode === 'image') {
    const custom = background.imageId === 'custom' ? backgroundCustom.src : null;
    const src = custom && isValidDataImage(custom) ? custom : findImage(background.imageId).src;
    return (
      `background-image: url("${cssUrl(src)}"); ` +
      `background-size: cover; background-position: center; background-repeat: no-repeat;`
    );
  }
  if (background.mode === 'pattern') {
    const pattern = findPattern(background.patternId);
    return `background-image: ${pattern.image}; background-size: ${pattern.size};`;
  }
  return '';
}

/**
 * Frost strength for the hero card, driven by the Dim slider: blur radius and
 * how much of var(--panel) is mixed into the card's background. The rest of
 * the photo stays untouched.
 */
export function frostStyle(): string {
  if (background.mode !== 'image') return '';
  const dim = background.dim;
  const blur = (4 + dim * 0.28).toFixed(1); // 0% → 4px, 85% → ~28px
  const alpha = Math.round(22 + dim * 0.5); // 0% → 22%, 85% → ~65%
  return `--frost-blur: ${blur}px; --frost-alpha: ${alpha}%;`;
}

/**
 * Tint for the terminal area. `termDim` 0 → fully opaque var(--panel) (the
 * default look); 85 → only 15% panel so the backdrop bleeds through. Also
 * drives a matching blur so the result reads as frosted glass, not noise.
 */
export function terminalTint(): { alpha: number; blur: number } {
  const alpha = Math.max(15, 100 - Math.round(background.termDim));
  const blur = Math.round(background.termDim * 0.32); // 85 → ~27px
  return { alpha, blur };
}
