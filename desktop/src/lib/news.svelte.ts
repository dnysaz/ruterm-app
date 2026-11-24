// What's New — loads release notes from news/*.md.
// Preference order per entry: fresh copy from GitHub raw (repo is public once
// released) → localStorage cache of the last fetch → bundled copy (offline
// fallback, generated into static/news at dev/build time by news/build-index.mjs).

export interface NewsEntry {
  /** File name inside news/, also the cache key. */
  file: string;
  date: string; // YYYY-MM-DD (from the file name)
  title: string;
  summary: string;
  /** SHA-256 of the Markdown body; required before remote/cache bodies render. */
  sha256: string;
  /** Markdown body, filled in once loaded. */
  markdown?: string;
  /** Where the body came from — useful for the "offline" hint in the UI. */
  source?: 'github' | 'cache' | 'bundle';
}

export interface NewsFeed {
  entries: NewsEntry[];
  source: 'github' | 'cache' | 'bundle' | 'empty';
}

// Mutable `main` could serve a matching index and body together. Pin to an
// immutable commit; bump this constant only when publishing a release.
export const NEWS_COMMIT = '89cb18f697b149388d576617d1aa8ecdd4016910';
const REPO_RAW = `https://raw.githubusercontent.com/dnysaz/ruterm-app/${NEWS_COMMIT}/news`;
const CACHE_KEY = 'rterm.news.cache.v1';
const LAST_SEEN_KEY = 'rterm.news.lastseen.v1';
const FETCH_TIMEOUT_MS = 3500;

// F-50: remote news is untrusted input. Cap every dimension before it reaches
// the Markdown renderer or localStorage.
const MAX_INDEX_ENTRIES = 100;
const MAX_FILE_BYTES = 256 * 1024;
const MAX_TITLE_CHARS = 200;
const MAX_SUMMARY_CHARS = 300;
const MAX_CACHE_BYTES = 2 * 1024 * 1024;
const NEWS_FILE_RE = /^\d{4}-\d{2}-\d{2}-[a-z0-9-]+\.md$/;
const DATE_RE = /^\d{4}-\d{2}-\d{2}$/;
const SHA256_RE = /^[a-f0-9]{64}$/i;

function byteLength(text: string): number {
  return new TextEncoder().encode(text).byteLength;
}

/** A body is only usable when it matches the digest in the pinned index. */
async function verifyBody(entry: Omit<NewsEntry, 'markdown' | 'source'>, body: string): Promise<boolean> {
  if (!SHA256_RE.test(entry.sha256 ?? '')) return false;
  if (byteLength(body) > MAX_FILE_BYTES) return false;
  const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(body));
  return Array.from(new Uint8Array(digest))
    .map(b => b.toString(16).padStart(2, '0'))
    .join('') === entry.sha256.toLowerCase();
}

export const newsState = $state({
  unread: 0,
  loaded: false,
});

interface CachePayload {
  fetched_at: string;
  index: { entries: Omit<NewsEntry, 'markdown' | 'source'>[] };
  files: Record<string, string>;
}

function readCache(): CachePayload | null {
  try {
    const raw = localStorage.getItem(CACHE_KEY);
    if (!raw || byteLength(raw) > MAX_CACHE_BYTES) return null;
    const parsed = JSON.parse(raw) as CachePayload;
    if (!parsed?.index?.entries || !parsed?.files || typeof parsed.files !== 'object') return null;
    if (!validIndex(parsed.index.entries)) return null;
    // Reject a poisoned cache whose bodies are oversized or path-traversal names.
    for (const [name, body] of Object.entries(parsed.files)) {
      if (!NEWS_FILE_RE.test(name) || typeof body !== 'string' || byteLength(body) > MAX_FILE_BYTES) {
        return null;
      }
    }
    return parsed;
  } catch {
    return null;
  }
}

function writeCache(payload: CachePayload) {
  try {
    const encoded = JSON.stringify(payload);
    if (byteLength(encoded) > MAX_CACHE_BYTES) return;
    localStorage.setItem(CACHE_KEY, encoded);
  } catch {
    /* storage full — non-fatal, remote fetch just happens again next launch */
  }
}

function readLastSeen(): string {
  try {
    return localStorage.getItem(LAST_SEEN_KEY) ?? '';
  } catch {
    return '';
  }
}

function writeLastSeen(iso: string) {
  try {
    localStorage.setItem(LAST_SEEN_KEY, iso);
  } catch {
    /* non-fatal */
  }
}

async function fetchWithTimeout(url: string): Promise<Response> {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), FETCH_TIMEOUT_MS);
  try {
    return await fetch(url, { signal: controller.signal, cache: 'no-cache' });
  } finally {
    clearTimeout(timer);
  }
}

async function fetchText(url: string): Promise<string> {
  const response = await fetchWithTimeout(url);
  if (!response.ok) throw new Error(`news: HTTP ${response.status}`);
  const declared = Number(response.headers.get('content-length') ?? '0');
  if (Number.isFinite(declared) && declared > MAX_FILE_BYTES) {
    throw new Error('news: body too large');
  }
  const text = await response.text();
  if (byteLength(text) > MAX_FILE_BYTES) throw new Error('news: body too large');
  return text;
}

async function fetchJson(url: string): Promise<{ entries?: Omit<NewsEntry, 'markdown' | 'source'>[] }> {
  const response = await fetchWithTimeout(url);
  if (!response.ok) throw new Error(`news: HTTP ${response.status}`);
  const text = await response.text();
  if (byteLength(text) > MAX_CACHE_BYTES) throw new Error('news: index too large');
  return JSON.parse(text) as { entries?: Omit<NewsEntry, 'markdown' | 'source'>[] };
}

function sortEntries(entries: NewsEntry[]): NewsEntry[] {
  return [...entries].sort((a, b) => (a.date < b.date ? 1 : a.date > b.date ? -1 : b.file.localeCompare(a.file)));
}

function validEntry(e: unknown): e is Omit<NewsEntry, 'markdown' | 'source'> {
  if (!e || typeof e !== 'object') return false;
  const v = e as Record<string, unknown>;
  return typeof v.file === 'string'
    && NEWS_FILE_RE.test(v.file)
    && typeof v.date === 'string'
    && DATE_RE.test(v.date)
    && !Number.isNaN(Date.parse(`${v.date}T00:00:00Z`))
    && typeof v.title === 'string' && v.title.length > 0 && v.title.length <= MAX_TITLE_CHARS
    && typeof v.sha256 === 'string' && SHA256_RE.test(v.sha256)
    && (v.summary === undefined
        || (typeof v.summary === 'string' && v.summary.length <= MAX_SUMMARY_CHARS));
}

function validIndex(value: unknown): value is Omit<NewsEntry, 'markdown' | 'source'>[] {
  return Array.isArray(value)
    && value.length <= MAX_INDEX_ENTRIES
    && value.every(validEntry);
}

async function fromBundle(): Promise<NewsFeed> {
  try {
    const index = await fetchJson('/news/index.json');
    const raw = validIndex(index.entries) ? index.entries : [];
    const entries = sortEntries(
      raw.map(e => ({ ...e, source: 'bundle' as const })),
    );
    await Promise.all(
      entries.map(async entry => {
        const body = await fetchText(`/news/${entry.file}`).catch(() => '');
        // Bundled copies are build-owned, but verify anyway so a tampered
        // static file can never reach the Markdown sink.
        entry.markdown = (await verifyBody(entry, body)) ? body : '';
      }),
    );
    return { entries, source: entries.length ? 'bundle' : 'empty' };
  } catch {
    return { entries: [], source: 'empty' };
  }
}

async function fromGithubOrCache(): Promise<NewsFeed | null> {
  // Fresh from the pinned commit…
  try {
    const index = await fetchJson(`${REPO_RAW}/index.json`);
    const raw = validIndex(index.entries) ? index.entries : null;
    if (!raw) throw new Error('news: malformed index.json');
    const files: Record<string, string> = {};
    const entries: NewsEntry[] = [];
    for (const e of raw) {
      const body = await fetchText(`${REPO_RAW}/${e.file}`).catch(() => '');
      // The index and body come from the same remote. A hash mismatch means
      // the feed is tampered: discard it instead of rendering or caching it.
      if (!await verifyBody(e, body)) throw new Error(`news: hash mismatch for ${e.file}`);
      files[e.file] = body;
      entries.push({ ...e, markdown: body, source: 'github' });
    }
    writeCache({ fetched_at: new Date().toISOString(), index: { entries: raw }, files });
    return { entries: sortEntries(entries), source: 'github' };
  } catch {
    // …or the cache of the last successful fetch, re-verified on every read.
    const cache = readCache();
    if (!cache || !validIndex(cache.index.entries)) return null;
    const entries: NewsEntry[] = [];
    for (const e of cache.index.entries) {
      const body = cache.files[e.file] ?? '';
      if (!await verifyBody(e, body)) return null;
      entries.push({ ...e, markdown: body, source: 'cache' });
    }
    return entries.length ? { entries: sortEntries(entries), source: 'cache' } : null;
  }
}

let inflight: Promise<NewsFeed> | null = null;

/** Merge bundle + remote. Remote wins per file; bundle fills what remote lacks. */
export async function loadNews(forceRefresh = false): Promise<NewsFeed> {
  if (inflight && !forceRefresh) return inflight;
  inflight = (async () => {
    const bundle = await fromBundle();
    const remote = await fromGithubOrCache();
    if (!remote) return bundle;

    const byFile = new Map<string, NewsEntry>();
    for (const entry of bundle.entries) byFile.set(entry.file, entry);
    for (const entry of remote.entries) byFile.set(entry.file, entry);
    const entries = sortEntries([...byFile.values()]);
    return { entries, source: remote.source };
  })();
  try {
    return await inflight;
  } finally {
    inflight = null;
  }
}

/** Unread = entry date newer than the last time the What's New tab was opened. */
export function unreadCount(entries: NewsEntry[]): number {
  const seen = readLastSeen();
  if (!seen) return entries.length;
  const seenTime = Date.parse(seen);
  if (Number.isNaN(seenTime)) return entries.length;
  return entries.filter(e => Date.parse(`${e.date}T00:00:00Z`) > seenTime).length;
}

/** Ids of entries newer than the last open — used for the dot next to each entry. */
export function loadUnreadIds(entries: NewsEntry[]): Set<string> {
  const seen = readLastSeen();
  const seenTime = seen ? Date.parse(seen) : 0;
  return new Set(
    entries
      .filter(e => Date.parse(`${e.date}T00:00:00Z`) > (Number.isNaN(seenTime) ? 0 : seenTime))
      .map(e => e.file),
  );
}

export function markAllRead(entries: NewsEntry[]) {
  const newest = entries.reduce((acc, e) => (e.date > acc ? e.date : acc), '');
  const stamp = newest ? `${newest}T23:59:59Z` : new Date().toISOString();
  writeLastSeen(stamp);
  newsState.unread = 0;
}

/** Called once at app start: fetch the feed and publish the unread badge count. */
export async function initNews() {
  newsState.loaded = false;
  try {
    const feed = await loadNews();
    newsState.unread = unreadCount(feed.entries);
  } catch {
    newsState.unread = 0;
  } finally {
    newsState.loaded = true;
  }
}
