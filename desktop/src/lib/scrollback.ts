/**
 * Terminal scrollback snapshots, kept on this device only.
 *
 * The live process cannot survive an app quit — PTYs die with the backend —
 * so RuTerm persists the *content* instead: what you read stays readable after
 * a restart, and SSH/local sessions reconnect into a fresh shell below it.
 *
 * The snapshot is plain text with ANSI escapes in localStorage. It never
 * syncs, never joins the vault, and can contain sensitive output — so it is
 * OFF by default, erased on lock/sign-out, and users must opt in explicitly.
 */

const KEY = 'rterm.scrollback.v1';
const PREF_KEY = 'rterm.restoreScrollback';

const MAX_TABS = 24;
const MAX_TAB_CHARS = 48 * 1024;
const MAX_TOTAL_CHARS = 512 * 1024;

export function isRestoreEnabled(): boolean {
  try {
    return localStorage.getItem(PREF_KEY) === 'on';
  } catch {
    return false;
  }
}

export function setRestoreEnabled(enabled: boolean) {
  try {
    localStorage.setItem(PREF_KEY, enabled ? 'on' : 'off');
    if (!enabled) localStorage.removeItem(KEY);
  } catch {
    /* private mode */
  }
}

function readAll(): Record<string, string> {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return {};
    const parsed: unknown = JSON.parse(raw);
    if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
      const out: Record<string, string> = {};
      for (const [id, value] of Object.entries(parsed as Record<string, unknown>)) {
        if (typeof value === 'string') out[id] = value;
      }
      return out;
    }
    return {};
  } catch {
    return {};
  }
}

export function readSnapshot(id: string): string | null {
  const all = readAll();
  return all[id] ?? null;
}

export function saveSnapshot(id: string, snapshot: string) {
  if (!isRestoreEnabled()) return;
  if (!snapshot) return;
  const all = readAll();
  // Re-insert so the freshest tabs survive eviction (string key order is stable).
  delete all[id];
  all[id] =
    snapshot.length > MAX_TAB_CHARS ? snapshot.slice(snapshot.length - MAX_TAB_CHARS) : snapshot;

  let keys = Object.keys(all);
  while (keys.length > MAX_TABS) {
    const oldest = keys.shift();
    if (oldest) delete all[oldest];
  }
  let total = Object.values(all).reduce((sum, value) => sum + value.length, 0);
  keys = Object.keys(all);
  while (total > MAX_TOTAL_CHARS && keys.length > 1) {
    const oldest = keys.shift();
    if (!oldest) break;
    total -= all[oldest]?.length ?? 0;
    delete all[oldest];
  }

  try {
    localStorage.setItem(KEY, JSON.stringify(all));
  } catch {
    // Quota exceeded: keep only the most recently saved handful and retry.
    try {
      const fresh: Record<string, string> = {};
      for (const key of Object.keys(all).slice(-4)) fresh[key] = all[key];
      localStorage.setItem(KEY, JSON.stringify(fresh));
    } catch {
      /* give up quietly */
    }
  }
}

export function removeSnapshot(id: string) {
  const all = readAll();
  if (!(id in all)) return;
  delete all[id];
  try {
    localStorage.setItem(KEY, JSON.stringify(all));
  } catch {
    /* ignore */
  }
}

/// Called when the vault is locked or the account is signed out: plaintext
/// terminal output must never outlive the unlocked session (F-49).
export function clearAllSnapshots() {
  try {
    localStorage.removeItem(KEY);
  } catch {
    /* ignore */
  }
}
