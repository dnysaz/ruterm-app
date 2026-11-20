// What's New — loads release notes from News/*.md.
// Preference order per entry: fresh copy from GitHub raw (repo is public once
// released) → localStorage cache of the last fetch → bundled copy (offline
// fallback, generated into static/news at dev/build time by News/build-index.mjs).

export interface NewsEntry {
  /** File name inside News/, also the cache key. */
  file: string;
  date: string; // YYYY-MM-DD (from the file name)
  title: string;
  summary: string;
  /** Markdown body, filled in once loaded. */
  markdown?: string;
  /** Where the body came from — useful for the "offline" hint in the UI. */
  source?: 'github' | 'cache' | 'bundle';
}

export interface NewsFeed {
  entries: NewsEntry[];
  source: 'github' | 'cache' | 'bundle' | 'empty';
}

const REPO_RAW = 'https://raw.githubusercontent.com/dnysaz/ruterm-app/main/News';
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

function byteLength(text: string): number {
  return new TextEncoder().encode(text).byteLength;
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
        entry.markdown = await fetchText(`/news/${entry.file}`).catch(() => '');
      }),
    );
    return { entries, source: entries.length ? 'bundle' : 'empty' };
  } catch {
    return { entries: [], source: 'empty' };
  }
}

async function fromGithubOrCache(): Promise<NewsFeed | null> {
  // Fresh from the repo…
  try {
    const index = await fetchJson(`${REPO_RAW}/index.json`);
    const raw = validIndex(index.entries) ? index.entries : null;
    if (!raw) throw new Error('news: malformed index.json');
    const files: Record<string, string> = {};
    const entries = await Promise.all(
      raw.map(async e => {
        try {
          files[e.file] = await fetchText(`${REPO_RAW}/${e.file}`);
          return { ...e, markdown: files[e.file], source: 'github' as const };
        } catch {
          // One missing file shouldn't hide the whole feed.
          return { ...e, markdown: '', source: 'github' as const };
        }
      }),
    );
    writeCache({ fetched_at: new Date().toISOString(), index: { entries: raw }, files });
    return { entries: sortEntries(entries), source: 'github' };
  } catch {
    // …or the cache of the last successful fetch.
    const cache = readCache();
    if (!cache) return null;
    const entries = validIndex(cache.index.entries)
      ? cache.index.entries.map(e => ({
          ...e,
          markdown: cache.files[e.file] ?? '',
          source: 'cache' as const,
        }))
      : [];
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
