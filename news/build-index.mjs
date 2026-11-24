#!/usr/bin/env node
/**
 * Scans news/*.md and regenerates:
 *   - news/index.json            (committed; the app fetches this from GitHub raw)
 *   - desktop/static/news/*.md   (bundled offline copy)
 *   - desktop/static/news/index.json
 *
 * Wired as predev/prebuild in desktop/package.json — no manual step needed.
 */
import { readdirSync, readFileSync, writeFileSync, mkdirSync, rmSync, copyFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = dirname(fileURLToPath(import.meta.url));
const staticDir = join(root, '..', 'desktop', 'static', 'news');

const files = readdirSync(root)
  .filter(f => /^\d{4}-\d{2}-\d{2}-.+\.md$/.test(f))
  .sort()
  .reverse(); // newest first

const entries = files.map(file => {
  const raw = readFileSync(join(root, file), 'utf8');
  const titleMatch = raw.match(/^#\s+(.+)$/m);
  const summaryMatch = raw
    .split('\n')
    .map(l => l.trim())
    .find(l => l && !l.startsWith('#') && !l.startsWith('```'));
  return {
    file,
    date: file.slice(0, 10),
    title: titleMatch ? titleMatch[1].trim() : file.replace(/\.md$/, ''),
    summary: summaryMatch ? summaryMatch.replace(/[#*`_]/g, '').slice(0, 160) : '',
    // Pinned remote news verifies each body against this digest before render.
    sha256: createHash('sha256').update(raw).digest('hex'),
  };
});

const index = { version: 1, generated_at: new Date().toISOString(), entries };
writeFileSync(join(root, 'index.json'), JSON.stringify(index, null, 2) + '\n');

rmSync(staticDir, { recursive: true, force: true });
mkdirSync(staticDir, { recursive: true });
for (const file of files) copyFileSync(join(root, file), join(staticDir, file));
writeFileSync(join(staticDir, 'index.json'), JSON.stringify(index, null, 2) + '\n');

console.log(`news: ${entries.length} entries -> news/index.json + desktop/static/news/`);
