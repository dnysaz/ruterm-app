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
    if (!raw) return null;
    const parsed = JSON.parse(raw) as CachePayload;
    if (!parsed?.index?.entries || !parsed?.files) return null;
    return parsed;
  } catch {
    return null;
  }
}

function writeCache(payload: CachePayload) {
  try {
    localStorage.setItem(CACHE_KEY, JSON.stringify(payload));
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
  if (!response.ok) throw new Error(`news: HTTP ${response.status} for ${url}`);
  return response.text();
}

async function fetchJson(url: string): Promise<{ entries?: Omit<NewsEntry, 'markdown' | 'source'>[] }> {
  const response = await fetchWithTimeout(url);
  if (!response.ok) throw new Error(`news: HTTP ${response.status} for ${url}`);
  return response.json();
}

function sortEntries(entries: NewsEntry[]): NewsEntry[] {
  return [...entries].sort((a, b) => (a.date < b.date ? 1 : a.date > b.date ? -1 : b.file.localeCompare(a.file)));
}

function validIndex(value: unknown): value is Omit<NewsEntry, 'markdown' | 'source'>[] {
  return Array.isArray(value) && value.every(e => e && typeof e.file === 'string' && typeof e.date === 'string' && typeof e.title === 'string');
}

async function fromBundle(): Promise<NewsFeed> {
  try {
    const index = await fetchJson('/news/index.json');
    const entries = sortEntries(
      (index.entries ?? []).map(e => ({ ...e, source: 'bundle' as const })),
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
