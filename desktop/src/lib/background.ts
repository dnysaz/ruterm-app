/**
 * Background catalog for the app shell (home + sidebar backdrop). Everything here is bundled
 * with the app — nothing is fetched from the network at runtime.
 *
 * Images live in `static/backgrounds/` (free-license photos, Unsplash —
 * https://unsplash.com/license). Patterns are pure CSS, adaptive to theme.
 *
 * To remove the whole feature: delete this file, `background.svelte.ts`,
 * `BackgroundSettings.svelte`, the `<BackgroundSettings />` line in
 * Settings.svelte and the `style` binding on `.empty-main` in
 * VaultShell.svelte.
 */

export type BackgroundMode = 'none' | 'image' | 'pattern';

export interface BackgroundImage {
  id: string;
  label: string;
  src: string;
}

export interface BackgroundPattern {
  id: string;
  label: string;
  /** CSS `background-image` value (works on var(--text) via color-mix). */
  image: string;
  /** CSS `background-size` value. */
  size: string;
}

export const BACKGROUND_IMAGES: BackgroundImage[] = [
  { id: 'mountains', label: 'Mountains', src: '/backgrounds/mountains.jpg' },
  { id: 'skyline', label: 'Skyline', src: '/backgrounds/city-night.jpg' },
  { id: 'street', label: 'Street', src: '/backgrounds/city.jpg' },
  { id: 'ocean', label: 'Ocean', src: '/backgrounds/ocean.jpg' },
  { id: 'forest', label: 'Forest', src: '/backgrounds/forest.jpg' },
  { id: 'desert', label: 'Desert', src: '/backgrounds/desert.jpg' },
  { id: 'beach', label: 'Beach', src: '/backgrounds/beach.jpg' },
  { id: 'circuit', label: 'Circuit', src: '/backgrounds/circuit.jpg' },
];

const line = (alpha: number) =>
  `color-mix(in srgb, var(--text) ${Math.round(alpha * 100)}%, transparent)`;
const accentLine = (alpha: number) =>
  `color-mix(in srgb, var(--accent) ${Math.round(alpha * 100)}%, transparent)`;

export const BACKGROUND_PATTERNS: BackgroundPattern[] = [
  {
    id: 'dots',
    label: 'Dots',
    image: `radial-gradient(${line(0.22)} 1.25px, transparent 1.4px)`,
    size: '24px 24px',
  },
  {
    id: 'blueprint',
    label: 'Blueprint',
    image: `linear-gradient(${accentLine(0.26)} 1px, transparent 1px), linear-gradient(90deg, ${accentLine(0.26)} 1px, transparent 1px)`,
    size: '24px 24px',
  },
  {
    id: 'diagonal',
    label: 'Diagonal',
    image: `repeating-linear-gradient(-45deg, ${line(0.1)} 0 1px, transparent 1px 15px)`,
    size: 'auto',
  },
  {
    id: 'accent-dots',
    label: 'Accent dots',
    image: `radial-gradient(${accentLine(0.5)} 1.25px, transparent 1.4px)`,
    size: '24px 24px',
  },
  {
    id: 'scanlines',
    label: 'Scanlines',
    image: `repeating-linear-gradient(0deg, ${line(0.12)} 0 1px, transparent 1px 9px)`,
    size: 'auto',
  },
  {
    id: 'grid',
    label: 'Grid',
    image: `linear-gradient(${line(0.14)} 1px, transparent 1px), linear-gradient(90deg, ${line(0.14)} 1px, transparent 1px)`,
    size: '22px 22px',
  },
];

export const DEFAULT_BACKDROP_IMAGE = BACKGROUND_IMAGES[0].id;
export const DEFAULT_PATTERN = BACKGROUND_PATTERNS[0].id;

export function findImage(id: string | undefined): BackgroundImage {
  return BACKGROUND_IMAGES.find(img => img.id === id) ?? BACKGROUND_IMAGES[0];
}

export function findPattern(id: string | undefined): BackgroundPattern {
  return BACKGROUND_PATTERNS.find(p => p.id === id) ?? BACKGROUND_PATTERNS[0];
}
