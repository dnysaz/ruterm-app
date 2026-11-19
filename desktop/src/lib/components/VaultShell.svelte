<script lang="ts">
  import { onMount } from 'svelte';
  import { api, newHost, type AiAgent, type Host, type Note, type SessionStatus } from '$lib/api';
  import { background, backgroundStyle, frostStyle } from '$lib/background.svelte';
  import {
    app,
    applyVaultPayload,
    deleteHost,
    deleteNote,
    fail,
    hosts,
    lock,
    notes,
    notify,
    saveHost,
    saveNote,
    syncNow,
    syncQuiet,
  } from '$lib/state.svelte';
  import HostAvatar from './HostAvatar.svelte';
  import HostForm from './HostForm.svelte';
  import Icon from './Icon.svelte';
  import { removeSnapshot } from '$lib/scrollback';
  import Settings from './Settings.svelte';
  import Terminal from './Terminal.svelte';
  import InfoPage from './InfoPage.svelte';
  import JsonEditor from './JsonEditor.svelte';
  import NoteEditor from './NoteEditor.svelte';
  import ConfirmDialog from './ConfirmDialog.svelte';
  import AiAgentIcon from './AiAgentIcon.svelte';
  import WhatsNew from './WhatsNew.svelte';
  import { initNews, newsState } from '$lib/news.svelte';

  interface AdhocTarget {
    username: string;
    address: string;
    port: number;
  }

  interface Tab {
    id: string;
    hostId: string;
    title: string;
    status: SessionStatus;
    kind:
      | 'ssh'
      | 'local'
      | 'settings'
      | 'hostform'
      | 'info'
      | 'whatsnew'
      | 'json'
      | 'note'
      | 'adhoc'
      | 'home';
    adhoc?: AdhocTarget;
    /** Local-agent tabs: shell starts here instead of $HOME. */
    cwd?: string;
    /** Local-agent tabs: command typed into the shell after spawn. */
    command?: string;
  }

  type InfoPageKind = 'about' | 'terms' | 'privacy' | 'contact' | 'github';

  const INFO_PAGES: Record<
    InfoPageKind,
    { title: string; icon: 'info' | 'file-text' | 'shield' | 'mail' | 'code' }
  > = {
    about: { title: 'About RuTerm', icon: 'info' },
    terms: { title: 'Terms & Conditions', icon: 'file-text' },
    privacy: { title: 'Privacy Policy', icon: 'shield' },
    contact: { title: 'Contact', icon: 'mail' },
    github: { title: 'GitHub', icon: 'code' },
  };

  type Zone = 'left' | 'right' | 'top' | 'bottom';

  interface Pane {
    id: 'a' | 'b';
    tabs: string[];
    active: string | null;
  }

  const PANE_A: 'a' = 'a';
  const PANE_B: 'b' = 'b';

  /** The Home tab is pinned first in pane A — it can be focused, never closed. */
  const HOME_ID = 'home';
  const HOME_TAB: Tab = {
    id: HOME_ID,
    hostId: '',
    title: 'Home',
    status: 'connected',
    kind: 'home',
  };

  let tabs = $state<Tab[]>([]);
  let panes = $state<Pane[]>([{ id: PANE_A, tabs: [], active: null }]);
  let splitAxis = $state<'row' | 'col'>('row');
  let splitFraction = $state(0.5);
  let focusedPaneId = $state<'a' | 'b'>(PANE_A);
  /** Draft seed for the singleton host-form tab (null until first opened). */
  let formHost = $state<Host | null>(null);
  let formEpoch = $state(0);
  let query = $state('');
  let filter = $state<'all' | 'favorite' | 'note'>('all');
  let noteToDelete = $state<Note | null>(null);
  let pickerOpen = $state(false);
  let collapsed = $state(readCollapsed());
  let dragTabId = $state<string | null>(null);
  let dropZone = $state<Zone | null>(null);
  let reorderHint = $state<{ paneId: string; beforeId: string | null } | null>(null);
  let tabMenu = $state<{ x: number; y: number; tabId: string } | null>(null);
  const tabMenuTab = $derived.by(() => {
    const menu = tabMenu;
    return menu ? (tabs.find(t => t.id === menu.tabId) ?? null) : null;
  });
  let appMenu = $state<{ x: number; y: number } | null>(null);

  // ---------------------------------------------------------- persistence
  const TABS_KEY = 'rterm.tabs.v1';
  let tabsReady = false;

  const clampFraction = (value: unknown): number =>
    typeof value === 'number' && Number.isFinite(value)
      ? Math.min(0.8, Math.max(0.2, value))
      : 0.5;

  const lastId = (ids: string[]): string | null => ids[ids.length - 1] ?? null;

  const fixPaneActive = (pane: Pane): Pane => ({
    ...pane,
    active: pane.active && pane.tabs.includes(pane.active) ? pane.active : lastId(pane.tabs),
  });

  /// Collapse to a single pane when one of the two empties out; otherwise keep both.
  function normalizePanes(list: Pane[]): Pane[] {
    const nonEmpty = list.filter(p => p.tabs.length > 0);
    if (nonEmpty.length === 0) return [{ id: PANE_A, tabs: [], active: null }];
    if (nonEmpty.length === 1) return [{ ...nonEmpty[0], id: PANE_A }];
    return list.slice(0, 2).map(fixPaneActive);
  }

  function applyPanes(next: Pane[]) {
    panes = normalizePanes(next.map(fixPaneActive));
    if (!panes.some(p => p.id === focusedPaneId)) focusedPaneId = panes[0].id;
  }

  function removeIdFromPanes(list: Pane[], id: string): Pane[] {
    return list.map(p => ({ ...p, tabs: p.tabs.filter(t => t !== id) }));
  }

  function insertIntoPane(list: Pane[], paneId: string, tabId: string, index: number | null): Pane[] {
    return list.map(p => {
      if (p.id !== paneId) return p;
      const next = [...p.tabs];
      let at = index === null ? next.length : Math.max(0, Math.min(index, next.length));
      // The pinned Home tab always keeps the first slot.
      if (next[0] === HOME_ID && at < 1) at = 1;
      next.splice(at, 0, tabId);
      return { ...p, tabs: next, active: tabId };
    });
  }

  function moveTabToPane(tabId: string, paneId: string, index: number | null) {
    const stripped = removeIdFromPanes(panes, tabId);
    const inserted = insertIntoPane(stripped, paneId, tabId, index);
    focusedPaneId = paneId as 'a' | 'b';
    applyPanes(inserted);
  }

  function paneIdOf(tabId: string): 'a' | 'b' | null {
    return panes.find(p => p.tabs.includes(tabId))?.id ?? null;
  }

  function lastFocusedOrFirst(): 'a' | 'b' {
    return panes.some(p => p.id === focusedPaneId) ? focusedPaneId : PANE_A;
  }

  /// Registers a brand-new (or existing) tab in the given pane and makes it active.
  function addTabToPane(tabId: string, paneId: 'a' | 'b') {
    moveTabToPane(tabId, paneId, null);
  }

  interface TabsSnapshot {
    tabs: Tab[];
    panes: Pane[];
    splitAxis: 'row' | 'col';
    splitFraction: number;
    focusedPane: 'a' | 'b';
  }

  function readTabs(): TabsSnapshot {
    const empty: TabsSnapshot = {
      tabs: [],
      panes: [{ id: PANE_A, tabs: [], active: null }],
      splitAxis: 'row',
      splitFraction: 0.5,
      focusedPane: PANE_A,
    };
    try {
      const raw = localStorage.getItem(TABS_KEY);
      if (!raw) return empty;
      const parsed = JSON.parse(raw);
      if (!Array.isArray(parsed?.tabs)) return empty;
      const safe: Tab[] = parsed.tabs
        .filter((t: unknown): t is Partial<Tab> & { id: string } => {
          // Page tabs (Settings, host form) are never restored — sessions and
          // notes only. Note bodies live in the vault; only the tab position is
          // snapshotted (hostId = note id).
          const kind = (t as Tab | null)?.kind;
          if (!t || typeof (t as Tab).id !== 'string') return false;
          if (kind === 'local' || kind === 'ssh') return true;
          if (kind === 'note') return typeof (t as Tab).hostId === 'string' && (t as Tab).hostId.length > 0;
          if (kind === 'adhoc') {
            const a = (t as Tab).adhoc;
            return (
              !!a &&
              typeof a.username === 'string' && a.username.length > 0 &&
              typeof a.address === 'string' && a.address.length > 0 &&
              typeof a.port === 'number' && Number.isInteger(a.port) && a.port >= 1 && a.port <= 65535
            );
          }
          return false;
        })
        .map((t: Partial<Tab> & { id: string }): Tab => {
          const adhoc =
            t.kind === 'adhoc' && t.adhoc
              ? {
                  username: String(t.adhoc.username),
                  address: String(t.adhoc.address),
                  port: Math.min(65535, Math.max(1, Math.floor(Number(t.adhoc.port) || 22))),
                }
              : undefined;
          return {
            id: t.id,
            hostId: typeof t.hostId === 'string' ? t.hostId : '',
            title: typeof t.title === 'string' ? t.title : adhoc ? `${adhoc.username}@${adhoc.address}` : 'Session',
            // The real status is resolved by session_attach on mount.
            status: t.kind === 'note' ? 'connected' : 'connecting',
            kind: t.kind === 'local' ? 'local' : t.kind === 'adhoc' ? 'adhoc' : t.kind === 'note' ? 'note' : 'ssh',
            ...(adhoc ? { adhoc } : {}),
            ...(typeof t.cwd === 'string' && t.kind === 'local' ? { cwd: t.cwd } : {}),
            ...(typeof t.command === 'string' && t.kind === 'local' ? { command: t.command } : {}),
          };
        });

      let restored: Pane[] = [];
      if (Array.isArray(parsed?.panes) && parsed.panes.length > 0) {
        const surviving = new Set(safe.map(t => t.id));
        const usedTabs = new Set<string>();
        const usedIds = new Set<'a' | 'b'>();
        for (const rawPane of parsed.panes.slice(0, 2) as unknown[]) {
          const src = (rawPane ?? {}) as Record<string, unknown>;
          let id: 'a' | 'b' = src.id === 'b' ? PANE_B : PANE_A;
          // Never emit duplicate pane ids or claim a tab twice.
          if (usedIds.has(id)) id = id === PANE_A ? PANE_B : PANE_A;
          if (usedIds.has(id)) continue;
          usedIds.add(id);
          const ids = Array.isArray(src.tabs)
            ? src.tabs.filter(
                (x: unknown): x is string =>
                  typeof x === 'string' && surviving.has(x) && !usedTabs.has(x),
              )
            : [];
          ids.forEach(x => usedTabs.add(x));
          restored.push(
            fixPaneActive({
              id,
              tabs: [...new Set(ids)],
              active: typeof src.active === 'string' ? src.active : null,
            }),
          );
        }
        // Any tab missing from the saved layout lands in the first pane.
        const covered = new Set(restored.flatMap(p => p.tabs));
        const leftovers = safe.filter(t => !covered.has(t.id)).map(t => t.id);
        if (leftovers.length > 0) {
          if (restored.length === 0) restored.push({ id: PANE_A, tabs: [], active: null });
          restored[0].tabs.push(...leftovers);
        }
        restored = normalizePanes(restored);
      } else {
        // Legacy v1 payload: one flat tab list + one active tab.
        restored = normalizePanes([
          fixPaneActive({
            id: PANE_A,
            tabs: safe.map(t => t.id),
            active: typeof parsed.activeTab === 'string' ? parsed.activeTab : null,
          }),
        ]);
      }

      const focused: 'a' | 'b' =
        parsed?.focusedPane === 'b' && restored.some(p => p.id === PANE_B) ? PANE_B : PANE_A;
      return {
        tabs: safe,
        panes: restored,
        splitAxis: parsed?.splitAxis === 'col' ? 'col' : 'row',
        splitFraction: clampFraction(parsed?.splitFraction),
        focusedPane: focused,
      };
    } catch {
      return empty;
    }
  }

  function saveTabs() {
    try {
      const snapshot = tabs
        // Home is pinned, page tabs are intentionally ephemeral. Adhoc tabs
        // persist connection details only — never credentials. Note tabs are
        // safe to restore: their body lives in the vault, never in localStorage.
        .filter(t => t.kind === 'local' || t.kind === 'ssh' || t.kind === 'adhoc' || t.kind === 'note')
        .map(t => ({
          id: t.id,
          hostId: t.hostId,
          title: t.title,
          kind: t.kind,
          ...(t.kind === 'local' && t.cwd ? { cwd: t.cwd } : {}),
          ...(t.kind === 'local' && t.command ? { command: t.command } : {}),
          ...(t.kind === 'adhoc' && t.adhoc ? { adhoc: t.adhoc } : {}),
        }));
      localStorage.setItem(
        TABS_KEY,
        JSON.stringify({
          tabs: snapshot,
          panes: panes.map(p => ({
            id: p.id,
            tabs: p.tabs.filter(id => {
              const tab = tabs.find(t => t.id === id);
              return !!tab && (tab.kind === 'ssh' || tab.kind === 'local' || tab.kind === 'adhoc' || tab.kind === 'note');
            }),
            active: p.active === HOME_ID ? null : p.active,
          })),
          splitAxis,
          splitFraction,
          focusedPane: focusedPaneId,
        }),
      );
    } catch {
      /* localStorage unavailable (private mode) — nothing to persist */
    }
  }

  $effect(() => {
    // Re-save whenever the layout changes.
    void tabs;
    void panes;
    void splitAxis;
    void splitFraction;
    void focusedPaneId;
    if (!tabsReady) return;
    saveTabs();
  });

  const visible = $derived(
    hosts()
      .filter(h => (filter === 'favorite' ? h.favorite : true))
      .filter(h => {
        const q = query.trim().toLowerCase();
        if (!q) return true;
        return (
          h.label.toLowerCase().includes(q) ||
          h.address.toLowerCase().includes(q) ||
          h.username.toLowerCase().includes(q) ||
          h.tags.some(t => t.toLowerCase().includes(q))
        );
      })
      .sort((a, b) => Number(b.favorite) - Number(a.favorite) || a.label.localeCompare(b.label)),
  );

  const noteList = $derived(
    notes()
      .filter(n => (filter === 'favorite' ? n.favorite : true))
      .filter(n => {
        const q = query.trim().toLowerCase();
        return !q || n.body.toLowerCase().includes(q);
      })
      .sort((a, b) => (b.updated_at || '').localeCompare(a.updated_at || '')),
  );

  const focusedPane = $derived(panes.find(p => p.id === focusedPaneId) ?? panes[0]);
  const focusedActiveTabId = $derived(focusedPane?.active ?? null);
  const activeTab = $derived(tabs.find(t => t.id === focusedActiveTabId) ?? null);
  const activeHostId = $derived(activeTab?.hostId ?? null);
  const activeNoteId = $derived(activeTab?.kind === 'note' ? activeTab.hostId : null);
  const tabById = $derived(new Map(tabs.map(t => [t.id, t] as const)));
  /** The About pill row only exists on the home tab — raise the AI dock above it then. */
  const homeVisible = $derived(panes.some(p => p.active === HOME_ID));
  // Recently opened hosts for the session picker (most recent first).
  const RECENT_HOSTS_KEY = 'rterm.recent.hosts';

  function readRecents(): string[] {
    try {
      if (typeof localStorage === 'undefined') return [];
      const raw = localStorage.getItem(RECENT_HOSTS_KEY);
      if (!raw) return [];
      const parsed = JSON.parse(raw);
      if (!Array.isArray(parsed)) return [];
      return parsed.filter(id => typeof id === 'string').slice(0, 10);
    } catch {
      return [];
    }
  }

  let recentHostIds = $state<string[]>(readRecents());

  function rememberHost(id: string) {
    recentHostIds = [id, ...recentHostIds.filter(x => x !== id)].slice(0, 10);
    try {
      localStorage.setItem(RECENT_HOSTS_KEY, JSON.stringify(recentHostIds));
    } catch {
      /* storage full/private mode — recents are best-effort */
    }
  }

  const recentHosts = $derived(
    recentHostIds
      .map(id => hosts().find(h => h.id === id))
      .filter((h): h is Host => !!h && !h.deleted)
      .slice(0, 5),
  );

  const pickable = $derived(
    hosts()
      .filter(h => !h.deleted)
      .sort(
        (a, b) =>
          Number(b.favorite) - Number(a.favorite) || a.label.localeCompare(b.label),
      ),
  );
  const account = $derived(app.status?.account ?? null);
  const accountName = $derived(account?.name ?? account?.email ?? 'Local mode');
  const accountInitial = $derived(accountName.slice(0, 1).toUpperCase());


  /**
   * Every click opens a new session — the same host may run in several tabs at
   * once (e.g. a long build in one, a shell in another).
   */
  function connect(host: Host) {
    pickerOpen = false;
    rememberHost(host.id);

    const base = host.label || host.address;
    const sameHost = tabs.filter(t => t.hostId === host.id).length;
    const title = sameHost > 0 ? `${base} ${sameHost + 1}` : base;

    // Doubles as the Tauri event channel: ssh://<id>.
    const tab: Tab = {
      id: crypto.randomUUID(),
      hostId: host.id,
      title,
      status: 'connecting',
      kind: 'ssh',
    };
    tabs = [...tabs, tab];
    addTabToPane(tab.id, lastFocusedOrFirst());
  }

  function readCollapsed(): boolean {
    try {
      return localStorage.getItem('rterm.sidebar') === 'collapsed';
    } catch {
      return false;
    }
  }

  function toggleSidebar() {
    collapsed = !collapsed;
    try {
      localStorage.setItem('rterm.sidebar', collapsed ? 'collapsed' : 'expanded');
    } catch {
      /* ignore */
    }
  }

  /** Opens the user's own shell in a PTY — useful for local work. */
  function openLocal() {
    pickerOpen = false;
    const count = tabs.filter(t => t.kind === 'local').length;
    const tab: Tab = {
      id: crypto.randomUUID(),
      hostId: '',
      title: count > 0 ? `Local shell ${count + 1}` : 'Local shell',
      status: 'connecting',
      kind: 'local',
    };
    tabs = [...tabs, tab];
    addTabToPane(tab.id, lastFocusedOrFirst());
  }

  // ---------------------------------------------------------------- notes
  /// A note's list title is the first few words of its body.
  function noteTitle(note: Note): string {
    const first = note.body
      .split('\n')
      .map(line => line.trim())
      .find(Boolean);
    if (!first) return 'Untitled note';
    const short = first.split(/\s+/).filter(Boolean).slice(0, 7).join(' ');
    return short.length > 56 ? `${short.slice(0, 55)}…` : short;
  }

  function noteTime(note: Note): string {
    const date = new Date(note.updated_at || note.created_at);
    if (Number.isNaN(date.getTime())) return '';
    return date.toLocaleString([], {
      month: 'short',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
    });
  }

  function openNote(note: Note) {
    pickerOpen = false;
    const id = `note-${note.id}`;
    const existing = tabs.find(t => t.id === id);
    if (existing) {
      activateTab(id);
      return;
    }
    const tab: Tab = {
      id,
      hostId: note.id,
      title: 'Note',
      status: 'connected',
      kind: 'note',
    };
    tabs = [...tabs, tab];
    addTabToPane(tab.id, lastFocusedOrFirst());
  }

  async function openNewNote() {
    if (!app.vault) return;
    const now = new Date().toISOString();
    const note: Note = {
      id: crypto.randomUUID(),
      body: '',
      favorite: false,
      created_at: now,
      updated_at: now,
      deleted: false,
    };
    try {
      await saveNote(note);
    } catch (error) {
      fail(error);
      return;
    }
    filter = 'note';
    openNote(note);
  }

  async function onNoteBody(note: Note, body: string) {
    try {
      await saveNote({ ...note, body });
      void syncQuiet();
    } catch (error) {
      fail(error);
    }
  }

  async function confirmDeleteNote() {
    const note = noteToDelete;
    noteToDelete = null;
    if (!note) return;
    try {
      await deleteNote(note.id);
      closeTab(`note-${note.id}`);
      void syncQuiet();
    } catch (error) {
      fail(error);
    }
  }

  // ------------------------------------------------------------ AI dock
  /// AI-agent CLIs installed on this machine (auto-detected). Clicking one
  /// opens a local shell in ~/Desktop and starts the agent right away.
  let aiAgents = $state<AiAgent[]>([]);

  function openAgent(agent: AiAgent) {
    pickerOpen = false;
    const tab: Tab = {
      id: crypto.randomUUID(),
      hostId: '',
      title: agent.label,
      status: 'connecting',
      kind: 'local',
      cwd: '~/Desktop',
      command: agent.command,
    };
    tabs = [...tabs, tab];
    addTabToPane(tab.id, lastFocusedOrFirst());
  }

  /** True while an agent tab is still starting up — drives the dock pulse. */
  function isAgentLaunching(agent: AiAgent): boolean {
    return tabs.some(
      t => t.kind === 'local' && t.command === agent.command && t.status === 'connecting',
    );
  }

  // ------------------------------------------------------------ quick connect
  /// Home-screen shortcut: type `ssh user@host[:port]` (or just `user@host`)
  /// and get a session without creating a vault host first.
  let quickTarget = $state('');
  let quickError = $state('');

  const QUICK_RE = /^\s*(?:ssh[ \t]+)?([A-Za-z0-9._-]+)@([A-Za-z0-9._-]+)(?::([0-9]{1,5}))?\s*$/;

  const quickParsed = $derived.by(() => {
    const match = quickTarget.trim().match(QUICK_RE);
    if (!match) return null;
    const port = match[3] ? Number(match[3]) : 22;
    if (port < 1 || port > 65535) return null;
    return { username: match[1], address: match[2], port };
  });

  function quickConnect(event?: SubmitEvent) {
    event?.preventDefault();
    const match = quickTarget.trim().match(QUICK_RE);
    if (!match) {
      quickError = 'Format: ssh username@host:port';
      return;
    }
    const port = match[3] ? Number(match[3]) : 22;
    if (port < 1 || port > 65535) {
      quickError = 'Port must be between 1 and 65535';
      return;
    }
    quickError = '';
    const tab: Tab = {
      id: crypto.randomUUID(),
      hostId: '',
      title: `${match[1]}@${match[2]}`,
      status: 'connecting',
      kind: 'adhoc',
      adhoc: {
        username: match[1],
        address: match[2],
        port,
      },
    };
    tabs = [...tabs, tab];
    addTabToPane(tab.id, lastFocusedOrFirst());
    quickTarget = '';
  }

  function cycleTab(direction: 1 | -1) {
    const pane = focusedPane;
    if (!pane || pane.tabs.length < 2) return;
    const index = pane.tabs.findIndex(id => id === pane.active);
    const next = (index + direction + pane.tabs.length) % pane.tabs.length;
    setPaneActive(pane.id, pane.tabs[next]);
  }

  function setPaneActive(paneId: 'a' | 'b', tabId: string | null) {
    panes = panes.map(p => (p.id === paneId ? { ...p, active: tabId } : p));
    focusedPaneId = paneId;
  }

  function openCount(hostId: string) {
    return tabs.filter(t => t.hostId === hostId).length;
  }

  /// Shared tab factory for SSH hosts and local shells.
  function spawnSibling(tab: Tab): Tab {
    if (tab.kind === 'local') {
      const count = tabs.filter(t => t.kind === 'local').length;
      return {
        id: crypto.randomUUID(),
        hostId: '',
        title: count > 0 ? `Local shell ${count + 1}` : 'Local shell',
        status: 'connecting',
        kind: 'local',
      };
    }
    const host = hosts().find(h => h.id === tab.hostId);
    const base = host ? host.label || host.address : tab.title.replace(/ \d+$/, '');
    const sameHost = tabs.filter(t => t.hostId === tab.hostId).length;
    return {
      id: crypto.randomUUID(),
      hostId: tab.hostId,
      title: sameHost > 0 ? `${base} ${sameHost + 1}` : base,
      status: 'connecting',
      kind: 'ssh',
    };
  }

  /** Open another session for the same host (same pane) from an existing tab. */
  function duplicateTab(tab: Tab) {
    if (tab.kind !== 'ssh' && tab.kind !== 'local') return; // page tabs have nothing to clone
    const sibling = spawnSibling(tab);
    tabs = [...tabs, sibling];
    addTabToPane(sibling.id, paneIdOf(tab.id) ?? lastFocusedOrFirst());
  }

  /// Page tabs (Settings, host form) are singletons that live in the tab bar like
  /// any session tab, so the vault behind them is never torn down.
  function isSessionTab(tab: Tab): boolean {
    return tab.kind === 'ssh' || tab.kind === 'local' || tab.kind === 'adhoc';
  }

  function terminalKind(tab: Tab): 'ssh' | 'local' | 'adhoc' {
    return tab.kind === 'local' ? 'local' : tab.kind === 'adhoc' ? 'adhoc' : 'ssh';
  }

  function paneIdOfTab(id: string): 'a' | 'b' | null {
    return panes.find(p => p.tabs.includes(id))?.id ?? null;
  }

  /** Focus an existing tab in whichever pane holds it. */
  function activateTab(id: string) {
    const paneId = paneIdOfTab(id);
    if (paneId) setPaneActive(paneId, id);
  }

  /** Open (or jump back to) the singleton Settings tab. */
  function openSettingsTab() {
    const existing = tabs.find(t => t.kind === 'settings');
    if (existing) {
      activateTab(existing.id);
      return;
    }
    const tab: Tab = {
      id: 'settings',
      hostId: '',
      title: 'Settings',
      status: 'connected',
      kind: 'settings',
    };
    tabs = [...tabs, tab];
    addTabToPane(tab.id, lastFocusedOrFirst());
  }

  /** Open (or reuse) a singleton info page tab — About, Terms, Privacy, Contact, GitHub. */
  function openInfoTab(page: InfoPageKind) {
    const id = `info-${page}`;
    const existing = tabs.find(t => t.id === id);
    if (existing) {
      activateTab(id);
      return;
    }
    const tab: Tab = {
      id,
      hostId: page,
      title: INFO_PAGES[page].title,
      status: 'connected',
      kind: 'info',
    };
    tabs = [...tabs, tab];
    addTabToPane(tab.id, lastFocusedOrFirst());
  }

  /** Open (or reuse) the singleton What's New tab. */
  function openWhatsNewTab() {
    const existing = tabs.find(t => t.id === 'whatsnew');
    if (existing) {
      activateTab('whatsnew');
      return;
    }
    const tab: Tab = {
      id: 'whatsnew',
      hostId: 'whatsnew',
      title: "What's New",
      status: 'connected',
      kind: 'whatsnew',
    };
    tabs = [...tabs, tab];
    addTabToPane(tab.id, lastFocusedOrFirst());
  }

  /** Open (or retarget and reuse) the singleton JSON editor tab. */
  function openJsonTab(kind: 'vault' | 'settings') {
    const existing = tabs.find(t => t.kind === 'json');
    if (existing) {
      if (existing.hostId !== kind) {
        tabs = tabs.map(t => (t.kind === 'json' ? { ...t, hostId: kind } : t));
      }
      activateTab(existing.id);
      return;
    }
    const tab: Tab = {
      id: 'json',
      hostId: kind,
      title: 'JSON editor',
      status: 'connected',
      kind: 'json',
    };
    tabs = [...tabs, tab];
    addTabToPane(tab.id, lastFocusedOrFirst());
  }

  /** Open (or reseed and reuse) the singleton host-form tab. */
  function openHostFormTab(host: Host | null) {
    const draft: Host = host ? JSON.parse(JSON.stringify(host)) : newHost();
    formHost = draft;
    formEpoch += 1;
    const title = host ? `Edit ${host.label || host.address}` : 'New host';
    const existing = tabs.find(t => t.kind === 'hostform');
    if (existing) {
      tabs = tabs.map(t => (t.id === 'hostform' ? { ...t, title, hostId: draft.id } : t));
      activateTab('hostform');
      return;
    }
    const tab: Tab = {
      id: 'hostform',
      hostId: draft.id,
      title,
      status: 'connected',
      kind: 'hostform',
    };
    tabs = [...tabs, tab];
    addTabToPane(tab.id, lastFocusedOrFirst());
  }

  // ------------------------------------------------------ app menu
  function openAppMenuAt(x: number, y: number) {
    const width = 198;
    const height = 220;
    appMenu = {
      x: Math.min(Math.max(8, x), Math.max(8, window.innerWidth - width - 8)),
      y: Math.min(Math.max(8, y), Math.max(8, window.innerHeight - height - 8)),
    };
    tabMenu = null;
  }

  function toggleAppMenu(button: HTMLElement) {
    if (appMenu) {
      appMenu = null;
      return;
    }
    const rect = button.getBoundingClientRect();
    openAppMenuAt(rect.left, rect.bottom + 8);
  }

  function closeAppMenu() {
    appMenu = null;
  }

  /// Focus the pinned Home tab — nothing is closed or unmounted.
  function goHome() {
    closeAppMenu();
    closeTabMenu();
    const pane = panes.find(p => p.tabs.includes(HOME_ID)) ?? panes[0];
    setPaneActive(pane.id, HOME_ID);
  }

  /// Right-click: real text fields keep the native edit menu; terminal and
  /// chrome (sidebar, tab bar, home) open the app menu at the pointer; document
  /// pages stay quiet.
  function onContextMenu(event: MouseEvent) {
    const target = event.target as HTMLElement | null;
    if (target?.closest('.xterm')) {
      event.preventDefault();
      openAppMenuAt(event.clientX, event.clientY);
      return;
    }
    if (target?.closest('input, textarea, select, [contenteditable="true"]')) return;
    event.preventDefault();
    if (target?.closest('.tab') || target?.closest('.tab-menu')) return;
    if (target?.closest('.pane')) return;
    openAppMenuAt(event.clientX, event.clientY);
  }

  // ------------------------------------------------------ tab context menu
  function openTabMenu(event: MouseEvent, tabId: string) {
    event.preventDefault();
    event.stopPropagation();
    closeAppMenu();
    // The Home tab is pinned — nothing to offer.
    if (tabId === HOME_ID) return;
    // Keep the menu inside the window even for right-clicks near an edge.
    const menuWidth = 228;
    const menuHeight = 296;
    const x = Math.min(Math.max(8, event.clientX), Math.max(8, window.innerWidth - menuWidth - 8));
    const y = Math.min(Math.max(8, event.clientY), Math.max(8, window.innerHeight - menuHeight - 8));
    tabMenu = { x, y, tabId };
  }

  function closeTabMenu() {
    tabMenu = null;
  }

  /// Split a tab to one side without dragging: existing layout moves the tab,
  /// a lone tab opens a fresh sibling session in the new pane (iTerm-style).
  function splitSession(tab: Tab, zone: Zone) {
    closeTabMenu();
    const total = panes.reduce((count, pane) => count + pane.tabs.length, 0);
    if (total > 1) {
      dropToZone(tab.id, zone);
      return;
    }
    // A lone page tab has nothing to pair the new pane with — leave layout alone.
    if (!isSessionTab(tab)) return;
    const axis: 'row' | 'col' = zone === 'left' || zone === 'right' ? 'row' : 'col';
    const toFirst = zone === 'left' || zone === 'top';
    splitAxis = axis;
    const sibling = spawnSibling(tab);
    tabs = [...tabs, sibling];
    const mine = { id: PANE_A, tabs: [tab.id], active: tab.id };
    const fresh = { id: PANE_B, tabs: [sibling.id], active: sibling.id };
    // Match dropToZone semantics: left/top keeps the tab first, right/bottom last.
    applyPanes(toFirst ? [mine, fresh] : [{ ...fresh, id: PANE_A }, { ...mine, id: PANE_B }]);
    focusedPaneId = toFirst ? PANE_A : PANE_B;
  }

  /// Close on outside pointer / Escape / window blur; Tab or arrows walk the items.
  function tabMenuBehavior(node: HTMLElement) {
    const items = () => [...node.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')];
    items()[0]?.focus();
    const onPointerDown = (event: PointerEvent) => {
      if (!node.contains(event.target as Node)) closeTabMenu();
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        event.stopPropagation();
        closeTabMenu();
        return;
      }
      const list = items();
      const index = list.indexOf(document.activeElement as HTMLButtonElement);
      if (event.key === 'ArrowDown') {
        event.preventDefault();
        list[(index + 1) % list.length]?.focus();
      } else if (event.key === 'ArrowUp') {
        event.preventDefault();
        list[(index - 1 + list.length) % list.length]?.focus();
      }
    };
    const onBlur = () => closeTabMenu();
    window.addEventListener('pointerdown', onPointerDown, true);
    window.addEventListener('keydown', onKey, true);
    window.addEventListener('blur', onBlur);
    window.addEventListener('resize', onBlur);
    return {
      destroy() {
        window.removeEventListener('pointerdown', onPointerDown, true);
        window.removeEventListener('keydown', onKey, true);
        window.removeEventListener('blur', onBlur);
        window.removeEventListener('resize', onBlur);
      },
    };
  }

  /// Same keyboard/outside-click contract as the tab menu.
  function appMenuBehavior(node: HTMLElement) {
    const items = () => [...node.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')];
    items()[0]?.focus();
    const onPointerDown = (event: PointerEvent) => {
      if (!node.contains(event.target as Node)) closeAppMenu();
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        event.stopPropagation();
        closeAppMenu();
        return;
      }
      const list = items();
      const index = list.indexOf(document.activeElement as HTMLButtonElement);
      if (event.key === 'ArrowDown') {
        event.preventDefault();
        list[(index + 1) % list.length]?.focus();
      } else if (event.key === 'ArrowUp') {
        event.preventDefault();
        list[(index - 1 + list.length) % list.length]?.focus();
      }
    };
    const onBlur = () => closeAppMenu();
    window.addEventListener('pointerdown', onPointerDown, true);
    window.addEventListener('keydown', onKey, true);
    window.addEventListener('blur', onBlur);
    window.addEventListener('resize', onBlur);
    return {
      destroy() {
        window.removeEventListener('pointerdown', onPointerDown, true);
        window.removeEventListener('keydown', onKey, true);
        window.removeEventListener('blur', onBlur);
        window.removeEventListener('resize', onBlur);
      },
    };
  }

  // ------------------------------------------------------------ tab drag & drop
  function onTabDragStart(event: DragEvent, tab: Tab) {
    if (tab.kind === 'home') return; // pinned — not draggable
    dragTabId = tab.id;
    if (event.dataTransfer) {
      event.dataTransfer.effectAllowed = 'move';
      event.dataTransfer.setData('text/plain', tab.id);
    }
    const src = paneIdOf(tab.id);
    if (src) focusedPaneId = src;
  }

  function clearDrag() {
    dragTabId = null;
    dropZone = null;
    reorderHint = null;
  }

  /// Elements of the same tab bar that can serve as re-insert landmarks.
  function reorderLandmarks(container: HTMLElement): HTMLElement[] {
    return [...container.querySelectorAll<HTMLElement>('.tab[data-tab-id]')].filter(
      el => el.dataset.tabId !== dragTabId && el.dataset.tabId !== HOME_ID,
    );
  }

  function computeReorderHint(container: HTMLElement, paneId: string, x: number) {
    let beforeId: string | null = null;
    for (const el of reorderLandmarks(container)) {
      const rect = el.getBoundingClientRect();
      if (x < rect.left + rect.width / 2) {
        beforeId = el.dataset.tabId ?? null;
        break;
      }
    }
    reorderHint = { paneId, beforeId };
  }

  function onTabsDragOver(event: DragEvent, paneId: 'a' | 'b') {
    if (!dragTabId) return;
    event.preventDefault();
    if (event.dataTransfer) event.dataTransfer.dropEffect = 'move';
    computeReorderHint(event.currentTarget as HTMLElement, paneId, event.clientX);
  }

  function onTabsDragLeave(event: DragEvent) {
    const related = event.relatedTarget as Node | null;
    if (!related || !(event.currentTarget as HTMLElement).contains(related)) {
      if (reorderHint) reorderHint = null;
    }
  }

  function onTabsDrop(event: DragEvent, paneId: 'a' | 'b') {
    if (!dragTabId) return;
    event.preventDefault();
    const hint = reorderHint;
    const beforeId = hint && hint.paneId === paneId ? hint.beforeId : null;
    let list = removeIdFromPanes(panes, dragTabId);
    const target = list.find(p => p.id === paneId);
    const index = beforeId === null || !target ? (target?.tabs.length ?? null) : target.tabs.indexOf(beforeId);
    list = insertIntoPane(list, paneId, dragTabId, index === -1 ? null : index);
    focusedPaneId = paneId;
    applyPanes(list);
    clearDrag();
  }

  function onZoneDragOver(event: DragEvent, zone: Zone) {
    if (!dragTabId) return;
    event.preventDefault();
    if (event.dataTransfer) event.dataTransfer.dropEffect = 'move';
    dropZone = zone;
  }

  function onZoneDrop(event: DragEvent, zone: Zone) {
    event.preventDefault();
    if (!dragTabId) return;
    dropToZone(dragTabId, zone);
  }

  /// Drops the tab onto a screen edge: creates (or re-orients) a two-pane split.
  function dropToZone(tabId: string, zone: Zone) {
    const axis: 'row' | 'col' = zone === 'left' || zone === 'right' ? 'row' : 'col';
    const toFirst = zone === 'left' || zone === 'top';
    splitAxis = axis;

    if (panes.length === 1) {
      const rest = panes[0].tabs.filter(t => t !== tabId);
      if (rest.length === 0) {
        // Splitting off the only tab would leave an empty half — ignore.
        clearDrag();
        return;
      }
      const first: Pane = toFirst
        ? { id: PANE_A, tabs: [tabId], active: tabId }
        : { id: PANE_A, tabs: rest, active: lastId(rest) };
      const second: Pane = toFirst
        ? { id: PANE_B, tabs: rest, active: lastId(rest) }
        : { id: PANE_B, tabs: [tabId], active: tabId };
      focusedPaneId = toFirst ? PANE_A : PANE_B;
      applyPanes([first, second]);
    } else {
      const target = toFirst ? panes[0].id : panes[1].id;
      moveTabToPane(tabId, target, null);
    }
    clearDrag();
  }

  /// Svelte actions: attach drag listeners without tripping a11y rules.
  /// (Drag-and-drop is pointer-only; the overlay is aria-hidden.)
  function dndTabs(node: HTMLElement, paneId: 'a' | 'b') {
    const over = (event: DragEvent) => onTabsDragOver(event, paneId);
    const leave = (event: DragEvent) => onTabsDragLeave(event);
    const drop = (event: DragEvent) => onTabsDrop(event, paneId);
    node.addEventListener('dragover', over);
    node.addEventListener('dragleave', leave);
    node.addEventListener('drop', drop);
    return {
      destroy() {
        node.removeEventListener('dragover', over);
        node.removeEventListener('dragleave', leave);
        node.removeEventListener('drop', drop);
      },
    };
  }

  function dndZone(node: HTMLElement, zone: Zone) {
    const over = (event: DragEvent) => onZoneDragOver(event, zone);
    const drop = (event: DragEvent) => onZoneDrop(event, zone);
    node.addEventListener('dragover', over);
    node.addEventListener('drop', drop);
    return {
      destroy() {
        node.removeEventListener('dragover', over);
        node.removeEventListener('drop', drop);
      },
    };
  }

  function trackPaneFocus(node: HTMLElement, paneId: 'a' | 'b') {
    const onFocusHint = () => {
      focusedPaneId = paneId;
    };
    node.addEventListener('pointerdown', onFocusHint, true);
    node.addEventListener('focusin', onFocusHint, true);
    return {
      destroy() {
        node.removeEventListener('pointerdown', onFocusHint, true);
        node.removeEventListener('focusin', onFocusHint, true);
      },
    };
  }

  /// Divider drag between the two panes; Pointer Events with capture.
  function startDividerDrag(event: PointerEvent) {    event.preventDefault();
    const handle = event.currentTarget as HTMLElement;
    const workspace = handle.parentElement;
    if (!workspace) return;
    try {
      handle.setPointerCapture(event.pointerId);
    } catch {
      /* synthetic or already-released pointers: drag still works via bubbling */
    }
    const move = (e: PointerEvent) => {
      const rect = workspace.getBoundingClientRect();
      const frac =
        splitAxis === 'row'
          ? (e.clientX - rect.left) / Math.max(rect.width, 1)
          : (e.clientY - rect.top) / Math.max(rect.height, 1);
      splitFraction = Math.min(0.8, Math.max(0.2, frac));
    };
    const stop = () => {
      handle.removeEventListener('pointermove', move);
      handle.removeEventListener('pointerup', stop);
      handle.removeEventListener('pointercancel', stop);
    };
    handle.addEventListener('pointermove', move);
    handle.addEventListener('pointerup', stop);
    handle.addEventListener('pointercancel', stop);
  }

  function onDividerKeydown(event: KeyboardEvent) {
    const step = 0.05;
    if (event.key === 'ArrowLeft' || event.key === 'ArrowUp') {
      splitFraction = Math.max(0.2, splitFraction - step);
      event.preventDefault();
    } else if (event.key === 'ArrowRight' || event.key === 'ArrowDown') {
      splitFraction = Math.min(0.8, splitFraction + step);
      event.preventDefault();
    }
  }

  /// Hosts we've already probed this run (unknown OS stays letter-avatar).
  const probeTried = new Set<string>();

  async function maybeProbe(hostId: string) {
    const host = hosts().find(h => h.id === hostId);
    if (!host) return;
    const known = (host.os ?? '').toLowerCase();
    if (known && known !== 'unknown') return;
    if (probeTried.has(hostId)) return;
    probeTried.add(hostId);
    try {
      const payload = await api.hostProbeOs(hostId);
      applyVaultPayload(payload);
    } catch {
      /* probe is best-effort; keep the letter avatar */
    }
  }

  function setStatus(id: string, status: SessionStatus) {
    const tab = tabs.find(t => t.id === id);
    if (!tab || tab.status === status) return;
    tabs = tabs.map(t => (t.id === id ? { ...t, status } : t));
    if (status === 'connected' && tab.kind === 'ssh') void maybeProbe(tab.hostId);
  }

  function startBlank() {
    openHostFormTab(null);
  }

  async function onSaved(host: Host) {
    await saveHost(host);
    notify('Host saved');
    closeTab('hostform');
  }

  async function onDelete(id: string) {
    await deleteHost(id);
    if (tabs.some(t => t.id === 'hostform')) closeTab('hostform');
    const doomed = new Set(tabs.filter(t => t.hostId === id).map(t => t.id));
    tabs = tabs.filter(t => !doomed.has(t.id));
    panes = panes.map(p => ({ ...p, tabs: p.tabs.filter(t => !doomed.has(t)) }));
    applyPanes(panes);
  }

  function closeTab(id: string) {
    if (id === HOME_ID) return; // the Home tab is pinned
    removeSnapshot(id);
    tabs = tabs.filter(t => t.id !== id);
    panes = panes.map(p => ({ ...p, tabs: p.tabs.filter(t => t !== id) }));
    applyPanes(panes);
  }

  /// Trust on first use: pin the key so the next connect can verify it.
  async function onFingerprint(hostId: string, fingerprint: string) {
    const host = hosts().find(h => h.id === hostId);
    if (!host || host.host_key_fingerprint === fingerprint) return;
    await saveHost({ ...host, host_key_fingerprint: fingerprint });
  }

  onMount(() => {
    // Show only the AI agents actually installed on this machine.
    void api.aiAgentsDetect().then(agents => (aiAgents = agents)).catch(() => (aiAgents = []));

    // WebView reload (Cmd/Ctrl+R) must not throw away open sessions.
    const restored = readTabs();
    if (restored.tabs.length) {
      const known = new Set(hosts().map(h => h.id));
      const liveNotes = new Set(notes().map(n => n.id));
      tabs = [
        HOME_TAB,
        ...restored.tabs.filter(t => {
          if (t.kind === 'local' || t.kind === 'adhoc') return true;
          // Note tabs only survive if their note still exists in the vault.
          if (t.kind === 'note') return liveNotes.has(t.hostId);
          return known.has(t.hostId);
        }),
      ];
      const surviving = new Set(tabs.map(t => t.id));
      panes = restored.panes.map((p, i) => ({
        ...p,
        // The pinned Home tab always sits first in the first pane.
        tabs: i === 0 ? [HOME_ID, ...p.tabs.filter(id => surviving.has(id))] : p.tabs.filter(id => surviving.has(id)),
      }));
      splitAxis = restored.splitAxis;
      splitFraction = restored.splitFraction;
      focusedPaneId = restored.focusedPane;
      applyPanes(panes);
    } else {
      tabs = [HOME_TAB];
      panes = [{ id: PANE_A, tabs: [HOME_ID], active: HOME_ID }];
      focusedPaneId = PANE_A;
    }
    tabsReady = true;
    void initNews();

    const onNext = () => cycleTab(1);
    const onPrev = () => cycleTab(-1);
    const onClose = () => {
      const current = focusedPane?.active ?? null;
      if (current) closeTab(current);
    };
    const onNew = () => (pickerOpen = true);

    window.addEventListener('rterm:next-tab', onNext);
    window.addEventListener('rterm:prev-tab', onPrev);
    window.addEventListener('rterm:close-tab', onClose);
    window.addEventListener('rterm:new-session', onNew);
    return () => {
      window.removeEventListener('rterm:next-tab', onNext);
      window.removeEventListener('rterm:prev-tab', onPrev);
      window.removeEventListener('rterm:close-tab', onClose);
      window.removeEventListener('rterm:new-session', onNew);
    };
  });

  function tabHost(tab: Tab): Host | null {
    if (tab.kind !== 'ssh') return null;
    return hosts().find(h => h.id === tab.hostId) ?? null;
  }

  function tabAddress(tab: Tab): string {
    if (tab.kind === 'local') return 'local pty';
    if (tab.kind === 'adhoc' && tab.adhoc) {
      return `${tab.adhoc.username}@${tab.adhoc.address}:${tab.adhoc.port}`;
    }
    if (tab.kind !== 'ssh') return '';
    const host = hosts().find(h => h.id === tab.hostId);
    return host ? `${host.username}@${host.address}:${host.port}` : '';
  }

</script>

<svelte:window oncontextmenu={onContextMenu} />

{#snippet addTabButton(paneId: 'a' | 'b')}
  <button
    class="add-tab"
    title="Open a session"
    aria-label="Open a session"
    onclick={() => {
      focusedPaneId = paneId;
      pickerOpen = !pickerOpen;
    }}
  >
    <Icon name="plus" size={14} />
  </button>
{/snippet}

<div class="shell" class:collapsed class:backdrop={background.mode !== 'none'} style={backgroundStyle()}>
  <aside>
    <header>
      {#if collapsed}
        <button
          class="brand-badge"
          type="button"
          title="Expand sidebar"
          aria-label="Expand sidebar"
          onclick={toggleSidebar}
        >
          <span class="badge-glyphs"><span class="r-letter">R</span><span class="r-caret" aria-hidden="true"></span></span>
        </button>
      {:else}
        <span class="wordmark">RuTerm<span class="cursor" aria-hidden="true">_</span></span>
        <span class="spacer"></span>
        <button
          class="icon-btn"
          title="Menu"
          aria-label="Menu"
          aria-haspopup="menu"
          aria-expanded={appMenu !== null}
          onclick={(event) => toggleAppMenu(event.currentTarget)}
        >
          <Icon name="ellipsis" size={16} />
        </button>
        <button
          class="icon-btn"
          title="Collapse sidebar"
          aria-label="Collapse sidebar"
          onclick={toggleSidebar}
        >
          <Icon name="panel-left" size={16} />
        </button>
      {/if}
    </header>

    {#if !collapsed}
      <div class="search">
        <span class="search-icon"><Icon name="search" size={18} /></span>
        <input autocapitalize="off" autocorrect="off" spellcheck="false" bind:value={query} placeholder="Search hosts" aria-label="Search hosts" />
      </div>

      <div class="filters">
        <button class:on={filter === 'all'} onclick={() => (filter = 'all')}>All</button>
        <button class:on={filter === 'favorite'} onclick={() => (filter = 'favorite')}>
          Favorite
        </button>
        <button class:on={filter === 'note'} onclick={() => (filter = 'note')}>
          Note
        </button>
        <span class="spacer"></span>
        <button
          class="icon-btn"
          title={filter === 'note' ? 'New note' : 'New host'}
          aria-label={filter === 'note' ? 'New note' : 'New host'}
          onclick={() => (filter === 'note' ? void openNewNote() : startBlank())}
        >
          <Icon name="plus" size={18} />
        </button>
      </div>
    {/if}

    <div class="list">
      {#if filter === 'note'}
        {#each noteList as note (note.id)}
          <div
            class="host note"
            class:active={activeNoteId === note.id}
            role="button"
            tabindex="0"
            title={note.body.slice(0, 120)}
            onclick={() => openNote(note)}
            onkeydown={event => event.key === 'Enter' && openNote(note)}
          >
            {#if !collapsed}
              <span class="meta">
                <span class="name">{noteTitle(note)}</span>
                <span class="sub">{noteTime(note)}</span>
              </span>
              {#if note.favorite}<span class="star" aria-label="Favorite">★</span>{/if}
              <button
                class="more"
                type="button"
                title="Delete note"
                aria-label="Delete note"
                onclick={event => {
                  event.stopPropagation();
                  noteToDelete = note;
                }}
              >
                <Icon name="trash" size={16} />
              </button>
            {:else}
              <span class="note-mark" aria-hidden="true">✎</span>
            {/if}
          </div>
        {:else}
          {#if !collapsed}
            <div class="empty">
              <p class="muted">{query ? 'No notes match your search.' : 'No notes yet.'}</p>
              <button class="btn sm" onclick={() => void openNewNote()}>New note</button>
            </div>
          {/if}
        {/each}

        {#if collapsed}
          <button
            class="rail-add"
            type="button"
            title="New note"
            aria-label="New note"
            onclick={() => void openNewNote()}
          >
            <Icon name="plus" size={18} />
          </button>
        {/if}
      {:else}
        {#each visible as host (host.id)}
        <div
          class="host"
          class:active={activeHostId === host.id}
          role="button"
          tabindex="0"
          title={collapsed ? `${host.label || host.address} — ${host.username}@${host.address}:${host.port}` : ''}
          onclick={() => connect(host)}
          onkeydown={event => event.key === 'Enter' && connect(host)}
          ondblclick={() => openHostFormTab(host)}
        >
          <HostAvatar host={host} size={collapsed ? 32 : 28} />
          {#if !collapsed}
            <span class="meta">
              <span class="name">
                {#if host.favorite}<span class="star" aria-label="Favorite">★</span>{/if}
                {host.label || host.address}
              </span>
              <span class="sub mono">{host.username}@{host.address}:{host.port}</span>
            </span>
            <button
              class="more"
              type="button"
              title="Edit host"
              aria-label="Edit host"
              onclick={event => {
                event.stopPropagation();
                openHostFormTab(host);
              }}
            >
              <Icon name="ellipsis" size={17} />
            </button>
          {/if}
        </div>
      {:else}
        {#if !collapsed}
          <div class="empty">
            <p class="muted">{query ? 'No hosts match your search.' : 'No hosts yet.'}</p>
            <button class="btn sm" onclick={startBlank}>Add host</button>
          </div>
        {/if}
        {/each}

        {#if collapsed}
          <button
            class="rail-add"
            type="button"
            title="New host"
            aria-label="New host"
            onclick={startBlank}
          >
            <Icon name="plus" size={18} />
          </button>
        {/if}
      {/if}
    </div>

    <footer>
      {#if collapsed}
        <button
          class="icon-btn"
          title="Local terminal"
          aria-label="Local terminal"
          onclick={openLocal}
        >
          <Icon name="terminal" size={18} />
        </button>
        <button
          class="icon-btn"
          title="Settings"
          aria-label="Settings"
          onclick={openSettingsTab}
        >
          <Icon name="settings" size={18} />
        </button>
        <button
          class="icon-btn"
          title="Sync now"
          aria-label="Sync now"
          disabled={app.busy || !app.status?.signed_in}
          onclick={() => void syncNow()}
        >
          <Icon name="sync" size={18} />
        </button>
        <button class="icon-btn" title="Lock vault" aria-label="Lock vault" onclick={() => void lock()}>
          <Icon name="lock" size={18} />
        </button>
      {:else}
        <div class="side-tools">
          <button class="side-action" type="button" onclick={openLocal} title="Open a local shell">
            <Icon name="terminal" size={15} stroke={2.3} />
            <span>Local terminal</span>
          </button>
          <button
            class="icon-btn"
            title="Settings"
            aria-label="Settings"
            onclick={openSettingsTab}
          >
            <Icon name="settings" size={16} stroke={2.1} />
          </button>
        </div>
        <div class="side-account">
          {#if account}
            {#if account.picture}
              <img class="who-avatar" src={account.picture} alt="" referrerpolicy="no-referrer" />
            {:else}
              <span class="who-avatar letter" aria-hidden="true">{accountInitial}</span>
            {/if}
          {:else}
            <span class="dot" class:signed={app.status?.signed_in}></span>
          {/if}
          <span class="who" title={accountName}>{accountName}</span>
          <span class="spacer"></span>
          <button
            class="icon-btn"
            title="Sync now"
            aria-label="Sync now"
            disabled={app.busy || !app.status?.signed_in}
            onclick={() => void syncNow()}
          >
            <Icon name="sync" size={15} stroke={2.3} />
          </button>
          <button class="icon-btn" title="Lock vault" aria-label="Lock vault" onclick={() => void lock()}>
            <Icon name="lock" size={15} stroke={2.3} />
          </button>
        </div>
      {/if}
    </footer>
  </aside>

  <main>
      <div class="workspace axis-{splitAxis}" class:split={panes.length === 2}>
        {#each panes as pane, i (pane.id)}
          {#if i > 0}
            <div
              class="divider axis-{splitAxis}"
              role="slider"
              aria-orientation={splitAxis === 'row' ? 'vertical' : 'horizontal'}
              aria-label="Resize split"
              aria-valuemin={20}
              aria-valuemax={80}
              aria-valuenow={Math.round(splitFraction * 100)}
              tabindex="0"
              onpointerdown={startDividerDrag}
              onkeydown={onDividerKeydown}
            ><span class="grip"></span></div>
          {/if}
          <div
            class="pane-wrap"
            class:focused={focusedPaneId === pane.id}
            style:flex={panes.length === 2
              ? `${(i === 0 ? splitFraction : 1 - splitFraction).toFixed(4)} 1 0%`
              : '1 1 0%'}
            use:trackPaneFocus={pane.id}
          >
            <div class="tabbar">
              <div
                class="tabs"
                class:drop-end={reorderHint?.paneId === pane.id && reorderHint.beforeId === null}
                role="tablist"
                tabindex="-1"
                use:dndTabs={pane.id}
              >
                {#each pane.tabs as id (id)}
                  {@const tab = tabById.get(id)}
                  {#if tab}
                    <div
                      class="tab"
                      class:active={pane.active === id}
                      class:dragging={dragTabId === id}
                      class:drop-before={reorderHint?.paneId === pane.id && reorderHint.beforeId === id}
                      role="tab"
                      tabindex="0"
                      aria-selected={pane.active === id}
                      draggable={tab.kind !== 'home'}
                      data-tab-id={id}
                      title={tab.kind === 'home'
                        ? 'Home'
                        : isSessionTab(tab)
                          ? `${tab.title} — drag to reorder, double-click for another session`
                          : `${tab.title} — drag to reorder`}
                      onclick={() => setPaneActive(pane.id, id)}
                      ondblclick={() => duplicateTab(tab)}
                      ondragstart={event => onTabDragStart(event, tab)}
                      ondragend={clearDrag}
                      oncontextmenu={event => openTabMenu(event, tab.id)}
                      onkeydown={event => event.key === 'Enter' && setPaneActive(pane.id, id)}
                    >
                      {#if tab.kind === 'home'}
                        <span class="tabicon page"><Icon name="home" size={13} /></span>
                      {:else if tab.kind === 'local'}
                        <span class="tabicon local"><Icon name="terminal" size={13} /></span>
                      {:else if tab.kind === 'settings'}
                        <span class="tabicon page"><Icon name="settings" size={13} /></span>
                      {:else if tab.kind === 'hostform'}
                        <span class="tabicon page"><Icon name={tab.title.startsWith('Edit') ? 'edit' : 'plus'} size={13} /></span>
                      {:else if tab.kind === 'info'}
                        <span class="tabicon page"><Icon name={INFO_PAGES[tab.hostId as InfoPageKind]?.icon ?? 'info'} size={13} /></span>
                      {:else if tab.kind === 'whatsnew'}
                        <span class="tabicon page"><Icon name="spark" size={13} /></span>
                      {:else if tab.kind === 'adhoc'}
                        <span class="tabicon local"><Icon name="terminal" size={13} /></span>
                      {:else if tab.kind === 'note'}
                        <span class="tabicon page"><Icon name="edit" size={13} /></span>
                      {:else}
                        <HostAvatar host={tabHost(tab)} size={16} />
                      {/if}
                      {#if tab.kind === 'ssh' || tab.kind === 'local'}
                        <span
                          class="tabdot"
                          class:ok={tab.status === 'connected'}
                          class:bad={tab.status === 'error'}
                          class:idle={tab.status === 'closed'}
                        ></span>
                      {/if}
                      <span class="tabname">{tab.title}</span>
                      {#if tab.kind !== 'home'}
                        <button
                          class="close"
                          type="button"
                          title="Close tab"
                          aria-label="Close tab"
                          onclick={event => {
                            event.stopPropagation();
                            closeTab(tab.id);
                          }}
                        >
                          <Icon name="close" size={12} />
                        </button>
                      {/if}
                    </div>
                  {/if}
                {/each}

                {@render addTabButton(pane.id)}
              </div>
            </div>

            <div class="panes">
              {#each pane.tabs as id (id)}
                {@const tab = tabById.get(id)}
                {#if tab}
                  <div class="pane" class:hidden={pane.active !== id}>
                    {#if tab.kind === 'home'}
                      <div class="empty-main home-panel">
        <div class="hero" class:frosted={background.mode === 'image'} style={frostStyle()}>
          <div class="eyebrow"><span class="hero-badge">R</span>RUTERM · SSH CLIENT</div>
          <h1>Connect in one line.</h1>
          <p class="lede">
            Type <code class="mono qc-code">ssh user@host:port</code> and hit Connect — no host setup
            needed. Saved hosts live in the sidebar.
          </p>

          <form class="quick-connect" onsubmit={quickConnect}>
            <label class="qc-label" for="quick-target">Quick connect</label>
            <div class="qc-row">
              <span class="qc-prefix mono" aria-hidden="true">ssh</span>
              <input
                id="quick-target"
                class="qc-input mono"
                bind:value={quickTarget}
                placeholder="user@10.0.0.5:22"
                autocapitalize="off"
                autocomplete="off"
                autocorrect="off"
                spellcheck="false"
              />
              <button class="btn primary qc-button" type="submit">
                Connect
                <Icon name="chevron-right" size={16} />
              </button>
            </div>
            <div class="qc-foot">
              {#if quickError}
                <p class="qc-error" role="alert">{quickError}</p>
              {:else if quickParsed}
                <p class="qc-parsed tiny mono">→ {quickParsed.username}@{quickParsed.address}:{quickParsed.port} · opens a new tab</p>
              {:else}
                <p class="qc-hint tiny muted">Opens directly in a new tab — the terminal will ask for a password if your SSH agent can’t sign in.</p>
              {/if}
            </div>
          </form>

          <div class="hero-actions">
            <button class="btn primary" onclick={startBlank}>
              <Icon name="plus" size={18} />
              New host
            </button>
            <button class="btn" onclick={openLocal} title="Open a shell on this device">
              <Icon name="terminal" size={17} />
              Local terminal
            </button>
            <button class="btn" onclick={openWhatsNewTab} title="Open release notes">
              <Icon name="spark" size={17} />
              What's New?
              {#if newsState.unread > 0}
                <span class="badge-count" aria-label={`${newsState.unread} unread release notes`}>
                  {newsState.unread > 9 ? '9+' : newsState.unread}
                </span>
              {/if}
            </button>
          </div>
        </div>

        <nav class="empty-links" aria-label="About RuTerm">
          <button type="button" onclick={() => openInfoTab('about')}>About</button>
          <button type="button" onclick={() => openInfoTab('terms')}>Terms</button>
          <button type="button" onclick={() => openInfoTab('privacy')}>Privacy</button>
          <button type="button" onclick={() => openInfoTab('contact')}>Contact</button>
          <button type="button" onclick={() => openInfoTab('github')}>GitHub</button>
        </nav>
                      </div>
                    {:else if tab.kind === 'settings'}
                      <Settings
                        onclose={() => closeTab(tab.id)}
                        onopeninfo={openInfoTab}
                        onopenwhatsnew={openWhatsNewTab}
                        onopenjson={openJsonTab}
                      />
                    {:else if tab.kind === 'json'}
                      {#key tab.hostId}
                        <JsonEditor
                          kind={tab.hostId === 'settings' ? 'settings' : 'vault'}
                          onclose={() => closeTab(tab.id)}
                        />
                      {/key}
                    {:else if tab.kind === 'info'}
                      <InfoPage
                        page={tab.hostId as InfoPageKind}
                        onclose={() => closeTab(tab.id)}
                      />
                    {:else if tab.kind === 'whatsnew'}
                      <WhatsNew onclose={() => closeTab(tab.id)} />
                    {:else if tab.kind === 'note'}
                      {#key tab.hostId}
                        <NoteEditor
                          note={notes().find(n => n.id === tab.hostId) ?? {
                            id: tab.hostId,
                            body: '',
                            favorite: false,
                            created_at: new Date().toISOString(),
                            updated_at: new Date().toISOString(),
                            deleted: false,
                          }}
                          onsave={body => {
                            const existing = notes().find(n => n.id === tab.hostId);
                            if (existing) void onNoteBody(existing, body);
                          }}
                          onclose={() => closeTab(tab.id)}
                        />
                      {/key}
                    {:else if tab.kind === 'hostform'}
                      {#key formEpoch}
                        {#if formHost}
                          <HostForm
                            host={formHost}
                            onclose={() => closeTab(tab.id)}
                            onsaved={onSaved}
                            ondelete={onDelete}
                          />
                        {/if}
                      {/key}
                    {:else}
                      <Terminal
                        sessionId={tab.id}
                        hostId={tab.hostId}
                        title={tab.title}
                        address={tabAddress(tab)}
                        kind={terminalKind(tab)}
                        adhoc={tab.adhoc}
                        cwd={tab.cwd}
                        command={tab.command}
                        onfingerprint={fingerprint => void onFingerprint(tab.hostId, fingerprint)}
                        onstatus={status => setStatus(tab.id, status)}
                      />
                    {/if}
                  </div>
                {/if}
              {/each}
            </div>
          </div>
        {/each}

        {#if dragTabId}
          <div class="drop-overlay" aria-hidden="true">
            <div
              class="zone zone-top"
              class:on={dropZone === 'top'}
              use:dndZone={'top'}
            >
              <span class="zone-label"><Icon name="rows" size={14} /> Split above</span>
            </div>
            <div
              class="zone zone-left"
              class:on={dropZone === 'left'}
              use:dndZone={'left'}
            >
              <span class="zone-label"><Icon name="columns" size={14} /> Split left</span>
            </div>
            <div class="zone-mid">
              <span>Drop on a tab bar to move — or on an edge to split</span>
            </div>
            <div
              class="zone zone-right"
              class:on={dropZone === 'right'}
              use:dndZone={'right'}
            >
              <span class="zone-label"><Icon name="columns" size={14} /> Split right</span>
            </div>
            <div
              class="zone zone-bottom"
              class:on={dropZone === 'bottom'}
              use:dndZone={'bottom'}
            >
              <span class="zone-label"><Icon name="rows" size={14} /> Split below</span>
            </div>
          </div>
        {/if}
      </div>

    {#if pickerOpen}
      <div
        class="picker-backdrop"
        role="presentation"
        onclick={() => (pickerOpen = false)}
      ></div>
      <div class="picker glass">
        <p class="picker-head">Open a session</p>

        <div class="picker-quick">
          <button class="picker-row action" type="button" onclick={openLocal}>
            <span class="action-icon terminal"><Icon name="terminal" size={15} /></span>
            <span class="meta">
              <span class="name">Local terminal</span>
              <span class="sub">Open a shell on this device</span>
            </span>
          </button>
          <button
            class="picker-row action"
            type="button"
            onclick={() => {
              pickerOpen = false;
              void openNewNote();
            }}
          >
            <span class="action-icon note"><Icon name="edit" size={15} /></span>
            <span class="meta">
              <span class="name">New note</span>
              <span class="sub">Write something to remember</span>
            </span>
          </button>
          <button
            class="picker-row action"
            type="button"
            onclick={() => {
              pickerOpen = false;
              startBlank();
            }}
          >
            <span class="action-icon plus"><Icon name="plus" size={15} /></span>
            <span class="meta">
              <span class="name">New host</span>
              <span class="sub">Add a server to the vault</span>
            </span>
          </button>
        </div>

        <div class="picker-sep"></div>
        <p class="picker-sub">Recent hosts</p>
        <div class="picker-list">
          {#each recentHosts as host (host.id)}
            <button class="picker-row" type="button" onclick={() => connect(host)}>
              <HostAvatar host={host} size={26} />
              <span class="meta">
                <span class="name">{host.label || host.address}</span>
                <span class="sub mono">{host.username}@{host.address}:{host.port}</span>
              </span>
              {#if openCount(host.id) > 0}
                <span class="badge">{openCount(host.id)}×</span>
              {/if}
            </button>
          {:else}
            {#each pickable.slice(0, 5) as host (host.id)}
              <button class="picker-row" type="button" onclick={() => connect(host)}>
                <HostAvatar host={host} size={26} />
                <span class="meta">
                  <span class="name">{host.label || host.address}</span>
                  <span class="sub mono">{host.username}@{host.address}:{host.port}</span>
                </span>
                {#if openCount(host.id) > 0}
                  <span class="badge">{openCount(host.id)}×</span>
                {/if}
              </button>
            {:else}
              <p class="muted tiny picker-empty">No hosts yet — add one with “New host”.</p>
            {/each}
          {/each}
        </div>
      </div>
    {/if}

    {#if tabMenu && tabMenuTab}
      {@const menuTab = tabMenuTab}
      <div
          class="tab-menu"
          role="menu"
          aria-label="Tab actions"
          style:left="{tabMenu.x}px"
          style:top="{tabMenu.y}px"
          use:tabMenuBehavior
        >
          <div class="tab-menu-label" title={menuTab.title}>{menuTab.title}</div>
          {#if isSessionTab(menuTab)}
          <button type="button" role="menuitem" title="Move this tab into a new pane on the left" onclick={() => splitSession(menuTab, 'left')}>
            <Icon name="panel-left" size={15} />
            Split Left
          </button>
          <button type="button" role="menuitem" title="Move this tab into a new pane on the right" onclick={() => splitSession(menuTab, 'right')}>
            <Icon name="panel-right" size={15} />
            Split Right
          </button>
          <button type="button" role="menuitem" title="Move this tab into a new pane above" onclick={() => splitSession(menuTab, 'top')}>
            <Icon name="panel-top" size={15} />
            Split Up
          </button>
          <button type="button" role="menuitem" title="Move this tab into a new pane below" onclick={() => splitSession(menuTab, 'bottom')}>
            <Icon name="panel-bottom" size={15} />
            Split Down
          </button>
          <div class="tab-menu-sep" role="separator"></div>
          <button type="button" role="menuitem" title="Open another session for the same host" onclick={() => { closeTabMenu(); duplicateTab(menuTab); }}>
            <Icon name="copy" size={15} />
            Duplicate Session
          </button>
          {/if}
          {#if menuTab.kind === 'note'}
            <button type="button" role="menuitem" class="danger" title="Delete this note" onclick={() => { closeTabMenu(); noteToDelete = menuTab.hostId ? notes().find(n => n.id === menuTab.hostId) ?? null : null; }}>
              <Icon name="trash" size={15} />
              Delete note
            </button>
          {/if}
          <button type="button" role="menuitem" class="danger" title="Close this session" onclick={() => { closeTabMenu(); closeTab(menuTab.id); }}>
            <Icon name="close" size={15} />
            Close Tab
          </button>
        </div>
    {/if}
  </main>

  {#if appMenu}
    <div
      class="tab-menu app-menu"
      role="menu"
      aria-label="RuTerm menu"
      style:left="{appMenu.x}px"
      style:top="{appMenu.y}px"
      use:appMenuBehavior
    >
      <button
        type="button"
        role="menuitem"
        onclick={() => {
          closeAppMenu();
          openHostFormTab(null);
        }}
      >
        <Icon name="plus" size={15} />
        New host
      </button>
      <button
        type="button"
        role="menuitem"
        onclick={() => {
          closeAppMenu();
          openSettingsTab();
        }}
      >
        <Icon name="settings" size={15} />
        Settings
      </button>
      <div class="tab-menu-sep" role="separator"></div>
      <button type="button" role="menuitem" onclick={goHome}>
        <Icon name="home" size={15} />
        Home
      </button>
      <button
        type="button"
        role="menuitem"
        onclick={() => {
          closeAppMenu();
          void lock();
        }}
      >
        <Icon name="lock" size={15} />
        Lock vault
      </button>
    </div>
  {/if}

  {#if aiAgents.length > 0}
    <div class="ai-dock" class:raised={homeVisible} role="toolbar" aria-label="Launch AI agents">
      {#each aiAgents as agent (agent.id)}
        <button
          class="ai-dock-btn"
          class:launching={isAgentLaunching(agent)}
          type="button"
          title={isAgentLaunching(agent) ? `Launching ${agent.label}…` : `${agent.label} — opens in ~/Desktop`}
          aria-label="Open {agent.label} in ~/Desktop"
          aria-busy={isAgentLaunching(agent)}
          onclick={() => openAgent(agent)}
        >
          <AiAgentIcon id={agent.id} size={24} />
        </button>
      {/each}
    </div>
  {/if}

  {#if noteToDelete}
    <ConfirmDialog
      title="Delete this note?"
      confirmLabel="Delete note"
      oncancel={() => (noteToDelete = null)}
      onconfirm={() => void confirmDeleteNote()}
    >
      <p>The note <strong>{noteTitle(noteToDelete)}</strong> will be removed from your vault — on this device and everywhere it syncs.</p>
    </ConfirmDialog>
  {/if}
</div>

<style>
  .shell {
    position: relative;
    height: 100%;
    display: grid;
    grid-template-columns: 280px minmax(0, 1fr);
    overflow: hidden;
    transition: grid-template-columns 0.18s ease;
  }
  .shell.collapsed {
    grid-template-columns: 64px minmax(0, 1fr);
  }
  /* With a backdrop picked in Settings, one photo/pattern spans the whole
     shell so it continues behind the translucent sidebar. */
  .shell.backdrop {
    background-color: var(--panel);
  }
  .shell.backdrop main {
    background: transparent;
  }
  .shell.backdrop .pane-wrap {
    background: transparent;
  }
  .shell.backdrop .panes {
    background: transparent;
  }
  .shell.backdrop .empty-main {
    background: transparent;
  }
  .shell.backdrop aside {
    background: color-mix(in srgb, var(--panel) 62%, transparent);
    backdrop-filter: blur(26px) saturate(135%);
    -webkit-backdrop-filter: blur(26px) saturate(135%);
  }

  aside {
    display: flex;
    flex-direction: column;
    min-height: 0;
    padding-top: 30px;
    border: 0;
    border-right: 1px solid var(--line);
    background: var(--panel);
    backdrop-filter: blur(42px) saturate(130%);
    -webkit-backdrop-filter: blur(42px) saturate(130%);
  }

  aside header {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 16px 10px 8px 14px;
  }
  .collapsed aside header {
    justify-content: center;
    padding: 16px 0 12px;
  }

  /* Collapsed rail: the mark is the whole brand. */
  .brand-badge {
    width: 36px;
    height: 36px;
    display: grid;
    place-items: center;
    padding: 0;
    border-radius: 50%;
    border: 1px solid var(--accent-dim);
    background: var(--accent-dim);
    color: var(--accent);
    font-family: var(--term);
    line-height: 1;
    cursor: pointer;
    transition:
      background 0.15s ease,
      border-color 0.15s ease;
  }
  .badge-glyphs {
    /* Lifts the pair ~1px so the low-hanging caret sits optically centered. */
    display: inline-flex;
    align-items: flex-end;
    gap: 1.5px;
    transform: translateY(-1px);
  }
  .r-letter {
    font-size: 14px;
    font-weight: 800;
    line-height: 1;
  }
  .r-caret {
    width: 0.52em;
    font-size: 14px;
    height: 2px;
    min-width: 7px;
    border-radius: 1px;
    background: var(--accent);
    margin-bottom: 2px;
    animation: caret 1.15s step-end infinite;
  }
  .brand-badge:hover {
    background: var(--accent-dim);
    border-color: var(--accent);
  }

  .wordmark {
    font-family: var(--sans);
    font-size: 15px;
    font-weight: 650;
    letter-spacing: -0.015em;
    color: var(--text);
    user-select: none;
  }
  .cursor {
    color: var(--accent);
    font-family: var(--term);
    font-weight: 700;
    margin-left: 1px;
    animation: caret 1.15s step-end infinite;
  }
  @keyframes caret {
    0%,
    55% {
      opacity: 1;
    }
    56%,
    100% {
      opacity: 0;
    }
  }

  aside .icon-btn {
    width: 34px;
    height: 34px;
    border-radius: 10px;
  }

  .search {
    position: relative;
    padding: 0 10px 8px;
  }
  .search-icon {
    position: absolute;
    left: 22px;
    top: 50%;
    transform: translateY(-60%);
    color: var(--text-tertiary);
    pointer-events: none;
    display: flex;
  }
  .search input {
    height: 36px;
    padding-left: 34px;
    font-size: 14px;
  }

  .filters {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 0 10px 10px;
  }
  .filters button:not(.icon-btn) {
    height: 28px;
    padding: 0 10px;
    border: 0;
    border-radius: 7px;
    background: transparent;
    color: var(--text-secondary);
    font-size: 13px;
    font-weight: 550;
    cursor: pointer;
  }
  .filters button:not(.icon-btn):hover {
    color: var(--text);
    background: var(--panel-hover);
  }
  .filters button.on {
    color: var(--text);
    background: var(--panel-hover);
  }

  .list {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 2px 8px 10px;
  }
  .collapsed .list {
    padding: 2px 8px 12px;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 4px;
  }
  .collapsed .host {
    padding: 3px;
    margin: 0;
    width: 100%;
    justify-content: center;
    background: transparent;
    border-color: transparent;
  }
  .collapsed .host:hover {
    background: var(--panel-hover);
  }
  .collapsed .host.active {
    background: var(--accent-dim);
    border-color: var(--accent-dim);
  }
  .rail-add {
    display: grid;
    place-items: center;
    width: 38px;
    height: 38px;
    margin-top: 4px;
    flex: 0 0 auto;
    border: 1px dashed var(--line);
    border-radius: 10px;
    background: transparent;
    color: var(--text-tertiary);
    cursor: pointer;
  }
  .rail-add:hover {
    color: var(--text);
    border-color: var(--line);
    background: var(--panel-hover);
  }
  .collapsed footer {
    flex-direction: column;
    align-items: center;
    gap: 4px;
    padding: 10px 0;
  }

  .host {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 9px;
    margin-bottom: 1px;
    border-radius: 10px;
    border: 1px solid transparent;
    cursor: pointer;
    position: relative;
  }
  .host:hover {
    background: var(--panel-hover);
  }
  .host.active {
    background: var(--accent-dim);
    border-color: var(--accent-dim);
  }
  .host.active::before {
    content: '';
    position: absolute;
    left: -1px;
    top: 50%;
    width: 3px;
    height: 16px;
    border-radius: 2px;
    background: var(--accent);
    transform: translateY(-50%);
  }
  .collapsed .host.active::before {
    display: none;
  }


  .meta {
    display: flex;
    flex-direction: column;
    gap: 1px;
    min-width: 0;
    flex: 1;
  }
  .name {
    font-size: 14px;
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    letter-spacing: -0.005em;
    color: var(--text);
  }
  .star {
    color: var(--warn);
    font-size: 11.5px;
    margin-right: 5px;
  }
  .sub {
    font-size: 12px;
    font-weight: 500;
    color: var(--text-secondary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .more {
    display: grid;
    place-items: center;
    width: 24px;
    height: 24px;
    border: 0;
    border-radius: 6px;
    background: transparent;
    color: var(--text-tertiary);
    opacity: 0;
    cursor: pointer;
  }
  .host:hover .more,
  .host.active .more {
    opacity: 1;
  }
  .more:hover {
    color: var(--text);
    background: var(--panel-hover);
  }

  .empty {
    padding: 34px 12px;
    text-align: center;
  }
  .empty p {
    margin: 0 0 12px;
    font-size: 13px;
  }

  aside footer {
    display: flex;
    flex-direction: column;
    align-items: stretch;
    gap: 2px;
    padding: 8px 10px 10px;
    border-top: 1px solid var(--line);
  }
  .side-tools {
    display: flex;
    align-items: center;
    gap: 2px;
  }
  .side-action {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: 1;
    min-width: 0;
    height: 28px;
    padding: 0 8px;
    border: 0;
    border-radius: 7px;
    background: transparent;
    color: var(--text);
    font-size: 12.5px;
    font-weight: 600;
    letter-spacing: -0.008em;
    text-align: left;
    cursor: pointer;
  }
  .side-action:hover {
    color: var(--text);
    background: var(--panel-hover);
  }
  .side-account {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 2px 6px 0 8px;
  }
  .dot {
    width: 7px;
    height: 7px;
    flex: 0 0 auto;
    border-radius: 50%;
    background: var(--text-tertiary);
  }
  .dot.signed {
    background: var(--ok);
  }
  .who-avatar {
    width: 22px;
    height: 22px;
    flex: 0 0 auto;
    border-radius: 50%;
    object-fit: cover;
    display: block;
    border: 1px solid var(--line);
  }
  .who-avatar.letter {
    display: grid;
    place-items: center;
    background: var(--panel-hover);
    font-size: 11px;
    font-weight: 700;
    letter-spacing: 0.02em;
    color: var(--text);
  }
  .who {
    font-size: 12px;
    font-weight: 650;
    letter-spacing: -0.008em;
    color: var(--text);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  main {
    position: relative;
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    background: var(--panel);
    overflow: hidden;
  }

  .empty-main {
    position: relative;
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    padding: 30px 44px 44px;
    overflow-y: auto;
    background: var(--panel);
  }
  /* Home tab lives inside a pane instead of owning the whole main area. */
  .empty-main.home-panel {
    height: 100%;
  }
  .hero {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 16px;
    width: 100%;
    max-width: 700px;
    text-align: center;
  }
  /* Frosted card behind the text/form: the photo stays sharp everywhere else. */
  .hero.frosted {
    max-width: 640px;
    padding: 34px 40px 30px;
    border-radius: 22px;
    border: 1px solid color-mix(in srgb, var(--text) 14%, transparent);
    background: color-mix(in srgb, var(--panel) var(--frost-alpha, 45%), transparent);
    backdrop-filter: blur(var(--frost-blur, 10px)) saturate(150%);
    -webkit-backdrop-filter: blur(var(--frost-blur, 10px)) saturate(150%);
  }
  @supports not ((backdrop-filter: blur(1px)) or (-webkit-backdrop-filter: blur(1px))) {
    .hero.frosted {
      background: color-mix(in srgb, var(--panel) 92%, transparent);
    }
  }
  .eyebrow {
    display: flex;
    align-items: center;
    gap: 12px;
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.16em;
    text-transform: uppercase;
    color: var(--text-tertiary);
    user-select: none;
  }
  .hero-badge {
    display: grid;
    place-items: center;
    width: 26px;
    height: 26px;
    border-radius: 7px;
    border: 1px solid var(--accent-dim);
    background: var(--accent-dim);
    color: var(--accent);
    font-family: var(--term);
    font-size: 12px;
    font-weight: 700;
    letter-spacing: 0;
  }
  .empty-main h1 {
    margin: 0;
    font-size: 26px;
    font-weight: 650;
    letter-spacing: -0.035em;
  }
  .lede {
    margin: 0;
    max-width: 50ch;
    color: var(--text-secondary);
    font-size: 13.5px;
    line-height: 1.6;
  }

  /* ---- quick connect card on the home screen ---- */
  .quick-connect {
    width: 100%;
    margin-top: 6px;
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 18px;
    border: 1px solid var(--line);
    border-radius: 16px;
    background: var(--bg);
    text-align: left;
  }
  .qc-label {
    margin: 0;
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.09em;
    text-transform: uppercase;
    color: var(--text-tertiary);
    user-select: none;
  }
  .qc-row {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .qc-prefix {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 42px;
    height: 44px;
    flex: 0 0 auto;
    border-radius: 10px;
    border: 1px solid var(--line);
    background: var(--bg-elevated);
    color: var(--text-secondary);
    font-size: 12.5px;
    font-weight: 600;
    letter-spacing: 0.02em;
    user-select: none;
  }
  .qc-input {
    flex: 1;
    height: 44px;
    min-width: 0;
    padding: 0 12px;
    border-radius: 10px;
    font-size: 13px;
  }
  .qc-input::placeholder {
    color: var(--text-tertiary);
    font-size: 12.5px;
  }
  .qc-button {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 44px;
    padding: 0 16px;
    flex: 0 0 auto;
    border-radius: 10px;
    font-size: 13px;
  }
  .qc-code {
    padding: 1px 6px;
    border: 1px solid var(--line);
    border-radius: 6px;
    background: var(--bg-elevated);
    font-size: 0.9em;
  }
  .qc-error {
    margin: 0;
    font-size: 12px;
    color: var(--bad);
  }
  .qc-parsed {
    margin: 0;
    color: var(--text-secondary);
    overflow-wrap: anywhere;
  }
  .qc-hint {
    margin: 0;
  }
  .qc-foot {
    min-height: 18px;
  }

  /* ------------------------------------------------- AI dock, bottom-right */
  .ai-dock {
    position: absolute;
    right: 16px;
    bottom: 16px;
    z-index: 29;
    display: flex;
    gap: 6px;
    align-items: center;
    padding: 6px;
    border: 1px solid var(--line);
    border-radius: 14px;
    background: var(--bg-elevated);
    transition: bottom 0.18s ease;
  }
  /* Clear of the About pill row, which only renders on the home tab. */
  .ai-dock.raised {
    bottom: 64px;
  }
  .ai-dock-btn {
    display: grid;
    place-items: center;
    width: 38px;
    height: 38px;
    border: 0;
    border-radius: 10px;
    background: transparent;
    cursor: pointer;
    transition: background 0.12s ease;
  }
  .ai-dock-btn:hover {
    background: var(--panel-hover);
  }
  .ai-dock-btn:active {
    background: var(--panel);
  }
  .ai-dock-btn:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  .ai-dock-btn.launching {
    background: var(--panel-hover);
  }
  .ai-dock-btn.launching :global(.ai-icon) {
    animation: aiLaunchPulse 1.15s ease-in-out infinite;
  }
  @keyframes aiLaunchPulse {
    0%,
    100% {
      opacity: 0.45;
      transform: scale(0.9);
    }
    50% {
      opacity: 1;
      transform: scale(1.05);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .ai-dock-btn.launching :global(.ai-icon) {
      animation: none;
    }
  }

  .hero-actions {
    display: flex;
    gap: 12px;
    margin-top: 2px;
  }
  .badge-count {
    display: inline-grid;
    place-items: center;
    min-width: 22px;
    height: 22px;
    padding: 0 6px;
    margin-left: 2px;
    border-radius: 999px;
    background: var(--bad);
    color: #fff;
    font-size: 10.5px;
    font-weight: 700;
    line-height: 1;
    user-select: none;
  }

  /* ---- footer links into the info tabs ---- */
  .empty-links {
    position: absolute;
    bottom: 0;
    right: 0;
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 8px;
    padding: 14px 18px;
    background: transparent;
  }
  .empty-links button {
    padding: 6px 14px;
    border: 1px solid color-mix(in srgb, var(--text) 12%, transparent);
    border-radius: 999px;
    background: color-mix(in srgb, var(--panel) 55%, transparent);
    backdrop-filter: blur(16px) saturate(140%);
    -webkit-backdrop-filter: blur(16px) saturate(140%);
    color: var(--text);
    font: inherit;
    font-size: 11.5px;
    font-weight: 500;
    letter-spacing: 0.02em;
    cursor: pointer;
    text-decoration: none;
    transition: color 0.14s ease, border-color 0.14s ease, background 0.14s ease;
  }
  .empty-links button:hover {
    color: var(--accent);
    border-color: color-mix(in srgb, var(--accent) 45%, transparent);
  }
  .empty-links button:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }

  .tabbar {
    position: relative;
    z-index: 25;
    display: flex;
    align-items: flex-end;
    height: 68px;
    padding: 30px 6px 0;
    flex: 0 0 auto;
    background: var(--tabbar);
  }
  .tabbar::after {
    content: '';
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    height: 1px;
    background: var(--line);
    z-index: 0;
  }

  .tabs {
    position: relative;
    z-index: 1;
    display: flex;
    align-items: flex-end;
    gap: 6px;
    height: 100%;
    min-width: 0;
    flex: 1;
    overflow-x: auto;
    scrollbar-width: none;
    padding: 0 6px;
  }
  .tabs::-webkit-scrollbar {
    display: none;
  }

  /* ---------- split workspace ---------- */
  .workspace {
    flex: 1;
    min-width: 0;
    min-height: 0;
    display: flex;
    position: relative;
  }
  .workspace.axis-row { flex-direction: row; }
  .workspace.axis-col { flex-direction: column; }
  .workspace.split .pane-wrap:not(.focused) .tabbar {
    filter: brightness(0.92);
  }
  .pane-wrap {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    position: relative;
    background: var(--panel);
  }

  .divider {
    flex: 0 0 auto;
    position: relative;
    z-index: 15;
    display: grid;
    place-items: center;
    background: transparent;
    border: 0;
    padding: 0;
  }
  .divider.axis-row { width: 7px; cursor: col-resize; }
  .divider.axis-col { height: 7px; cursor: row-resize; }
  .divider::before {
    content: '';
    background: var(--line);
    transition: background 0.15s ease;
  }
  .divider.axis-row::before { width: 1px; height: 100%; }
  .divider.axis-col::before { height: 1px; width: 100%; }
  .divider .grip {
    position: absolute;
    border-radius: 999px;
    background: var(--line);
    transition: background 0.15s ease;
  }
  .divider.axis-row .grip { width: 3px; height: 30px; }
  .divider.axis-col .grip { height: 3px; width: 30px; }
  .divider:hover::before,
  .divider:focus-visible::before,
  .divider:hover .grip,
  .divider:focus-visible .grip {
    background: var(--accent);
  }
  .divider:focus-visible { outline: none; }

  .tab { cursor: grab; }
  .tab:active { cursor: grabbing; }
  .tab.dragging { opacity: 0.45; }
  .tab.drop-before { box-shadow: -2px 0 0 0 var(--accent); }
  .tabs.drop-end::after {
    content: '';
    align-self: flex-end;
    flex: 0 0 auto;
    width: 2px;
    height: 26px;
    margin-bottom: 4px;
    border-radius: 1px;
    background: var(--accent);
    margin-left: 1px;
  }

  /* ---------- tab context menu ---------- */
  .tab-menu {
    position: fixed;
    z-index: 60;
    min-width: 216px;
    padding: 6px;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: 12px;
    backdrop-filter: blur(16px) saturate(1.15);
    -webkit-backdrop-filter: blur(16px) saturate(1.15);
  }
  .tab-menu-label {
    padding: 5px 9px 7px;
    font-size: 11.5px;
    font-weight: 650;
    letter-spacing: 0.09em;
    text-transform: uppercase;
    color: var(--text-secondary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 224px;
  }
  .tab-menu [role='menuitem'] {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    padding: 7px 9px;
    border: 0;
    border-radius: 8px;
    background: transparent;
    color: var(--text);
    font: inherit;
    font-size: 13.5px;
    text-align: left;
    cursor: pointer;
  }
  .tab-menu [role='menuitem']:hover,
  .tab-menu [role='menuitem']:focus-visible {
    background: var(--panel-hover);
    outline: none;
  }
  .tab-menu [role='menuitem'] :global(svg) {
    color: var(--text-secondary);
    flex: 0 0 auto;
  }
  .tab-menu-sep {
    height: 1px;
    margin: 5px 6px;
    background: var(--line);
  }
  .tab-menu .danger {
    color: var(--bad);
  }
  .tab-menu .danger:hover,
  .tab-menu .danger:focus-visible {
    background: rgba(239, 111, 92, 0.13);
  }
  .tab-menu .danger :global(svg) {
    color: var(--bad);
  }

  /* ---------- app menu ---------- */
  .tab-menu.app-menu {
    min-width: 198px;
    z-index: 70;
  }

  .drop-overlay {
    position: absolute;
    z-index: 18;
    inset: 41px 0 0 0;
    display: grid;
    grid-template-columns: 1fr 1.4fr 1fr;
    grid-template-rows: 1fr 1.4fr 1fr;
    gap: 10px;
    padding: 12px;
    background: color-mix(in srgb, var(--panel) 45%, transparent);
    backdrop-filter: blur(3px);
    -webkit-backdrop-filter: blur(3px);
  }
  .zone {
    display: grid;
    place-items: center;
    border: 1.5px dashed var(--accent-dim);
    border-radius: 14px;
    background: color-mix(in srgb, var(--accent) 5%, transparent);
    transition:
      background 0.12s ease,
      border-color 0.12s ease;
  }
  .zone.on {
    background: color-mix(in srgb, var(--accent) 16%, transparent);
    border: 1.5px solid var(--accent);
  }
  .zone-top { grid-area: 1 / 1 / 2 / 4; }
  .zone-left { grid-area: 2 / 1; }
  .zone-right { grid-area: 2 / 3; }
  .zone-bottom { grid-area: 3 / 1 / 4 / 4; }
  .zone-mid {
    grid-area: 2 / 2;
    display: grid;
    place-items: center;
    pointer-events: none;
  }
  .zone-mid span {
    font-size: 12.5px;
    color: var(--text-secondary);
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: 999px;
    padding: 8px 16px;
    max-width: 34ch;
    text-align: center;
  }
  .zone-label {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    font-size: 11px;
    letter-spacing: 0.12em;
    text-transform: uppercase;
    color: var(--accent);
    background: color-mix(in srgb, var(--accent) 10%, rgba(0, 0, 0, 0.35));
    border: 1px solid var(--accent-dim);
    border-radius: 999px;
    padding: 7px 14px;
  }



  /* Folder tabs: rounded shoulders that flare into the tab strip, joined to
     the pane below so the active tab reads as the front of a folder. */
  .tab {
    display: flex;
    align-items: center;
    gap: 8px;
    max-width: 240px;
    height: 36px;
    padding: 0 6px 0 12px;
    border: 0;
    border-radius: 11px 11px 0 0;
    background: transparent;
    color: var(--text-secondary);
    font-size: 13px;
    cursor: pointer;
    white-space: nowrap;
    flex: 0 0 auto;
    margin-bottom: 0;
    position: relative;
    transition:
      background 0.14s ease,
      color 0.14s ease;
  }
  .tab:hover {
    color: var(--text);
    background: var(--panel-hover);
  }
  .tab.active {
    color: var(--text);
    background: var(--panel);
  }
  .tab.active::before,
  .tab.active::after {
    content: '';
    position: absolute;
    bottom: 0;
    width: 6px;
    height: 6px;
    pointer-events: none;
    background: radial-gradient(circle at 0 0, transparent 0 5px, var(--panel) 5.5px);
  }
  .tab.active::before {
    right: 100%;
  }
  .tab.active::after {
    left: 100%;
    transform: scaleX(-1);
  }

  .tabicon.local {
    flex: 0 0 auto;
    display: grid;
    place-items: center;
    width: 16px;
    height: 16px;
    border-radius: 5px;
    color: var(--text-secondary);
  }
  .tab.active .tabicon.local {
    color: var(--accent);
  }
  .tabicon.page {
    flex: 0 0 auto;
    display: grid;
    place-items: center;
    width: 16px;
    height: 16px;
    border-radius: 5px;
    color: var(--text-secondary);
  }
  .tab.active .tabicon.page {
    color: var(--accent);
  }

  .tabdot {
    width: 6px;
    height: 6px;
    flex: 0 0 auto;
    border-radius: 50%;
    background: var(--warn);
  }
  .tabdot.ok {
    background: var(--ok);
  }
  .tabdot.bad {
    background: var(--bad);
  }
  .tabdot.idle {
    background: var(--text-tertiary);
  }
  .tabname {
    overflow: hidden;
    text-overflow: ellipsis;
    font-weight: 500;
  }
  .close {
    display: grid;
    place-items: center;
    width: 19px;
    height: 19px;
    padding: 0;
    flex: 0 0 auto;
    border: 0;
    border-radius: 5px;
    background: transparent;
    color: var(--text-tertiary);
    opacity: 0;
    cursor: pointer;
  }
  .tab:hover .close,
  .tab.active .close {
    opacity: 1;
  }
  .close:hover {
    color: var(--text);
    background: var(--panel-hover);
  }

  .add-tab {
    display: grid;
    place-items: center;
    width: 28px;
    height: 28px;
    margin: 0 0 4px 3px;
    flex: 0 0 auto;
    border: 0;
    border-radius: 7px;
    background: transparent;
    color: var(--text-tertiary);
    cursor: pointer;
  }
  .add-tab:hover {
    color: var(--text);
    background: var(--panel-hover);
  }

  .picker-backdrop {
    position: fixed;
    inset: 0;
    z-index: 30;
  }
  .picker {
    position: absolute;
    top: 76px;
    right: 12px;
    z-index: 31;
    width: 306px;
    border-radius: 13px;
    padding: 6px;
    background: var(--panel);
    border: 1px solid var(--line);
  }
  .picker-head {
    margin: 4px 8px 6px;
    font-size: 11.5px;
    letter-spacing: 0.09em;
    text-transform: uppercase;
    color: var(--text-tertiary);
  }
  .picker-list {
    max-height: 300px;
    overflow-y: auto;
  }
  .picker-row {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    padding: 7px 8px;
    border: 0;
    border-radius: 9px;
    background: transparent;
    color: var(--text);
    text-align: left;
    cursor: pointer;
  }
  .picker-row:hover {
    background: var(--panel-hover);
  }
  .picker-empty {
    margin: 8px;
  }
  .badge {
    font-size: 10.5px;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--text-tertiary);
    border: 1px solid var(--line);
    border-radius: 5px;
    padding: 1px 5px;
    flex: 0 0 auto;
  }
  .picker-quick {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .action-icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    border-radius: 8px;
    background: var(--bg-elevated);
    border: 1px solid var(--line);
    color: var(--text-secondary);
    flex: 0 0 auto;
  }
  .picker-row.action:hover .action-icon {
    color: var(--text);
    border-color: var(--line-hover);
  }
  .picker-sep {
    height: 1px;
    background: var(--line);
    margin: 6px 2px;
  }
  .picker-sub {
    margin: 6px 8px 4px;
    font-size: 11px;
    letter-spacing: 0.09em;
    text-transform: uppercase;
    color: var(--text-tertiary);
  }

  .panes {
    flex: 1;
    min-height: 0;
    position: relative;
    background: var(--panel);
  }
  .pane {
    position: absolute;
    inset: 0;
  }
  .pane.hidden {
    display: none;
  }
</style>
