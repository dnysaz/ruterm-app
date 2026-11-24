<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import {
    api,
    newHost,
    type AgentLaunch,
    type AiAgent,
    type Host,
    type Note,
    type SessionStatus,
    type SftpEntry,
    type VaultPayload,
  } from '#lib/api';
  import { background, backgroundStyle, frostStyle } from '#lib/background.svelte';
  import { imageMimeForPath } from '#lib/images';
  import { noteTitle } from '#lib/note-title';
  import {
    app,
    applyVaultPayload,
    changeAccount,
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
  } from '#lib/state.svelte';
  import HostAvatar from './HostAvatar.svelte';
  import HostForm from './HostForm.svelte';
  import Icon from './Icon.svelte';
  import SftpPanel from './SftpPanel.svelte';
  import SftpFileView from './SftpFileView.svelte';
  import ImagePreview from './ImagePreview.svelte';
  import { removeSnapshot } from '#lib/scrollback';
  import Settings from './Settings.svelte';
  import Terminal from './Terminal.svelte';
  import InfoPage from './InfoPage.svelte';
  import NoteEditor from './NoteEditor.svelte';
  import ConfirmDialog from './ConfirmDialog.svelte';
  import TrafficLights from './TrafficLights.svelte';
  import AiAgentIcon from './AiAgentIcon.svelte';
  import WhatsNew from './WhatsNew.svelte';
  import AiChatDrawer from './AiChatDrawer.svelte';
  import { initNews, newsState } from '#lib/news.svelte';

  interface AdhocTarget {
    username: string;
    address: string;
    port: number;
    /** Explicit consent to offer the local SSH agent to this host (F-06). */
    allowAgent: boolean;
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
      | 'note'
      | 'adhoc'
      | 'sftp'
      | 'sftpfile'
      | 'home'
      | 'chat';
    adhoc?: AdhocTarget;
    /** SFTP file-viewer tabs: the parent SFTP session's backend key. */
    sftpId?: string;
    /** SFTP file-viewer tabs: absolute remote path of the file. */
    filePath?: string;
    /** Local-agent tabs: shell starts here instead of $HOME. */
    cwd?: string;
    /** Local-agent tabs: command typed into the shell after spawn. */
    command?: string;
    /** Local-agent tabs: environment injected into the PTY (MCP bridge). */
    env?: Record<string, string>;
    /** Local-agent tabs: id used to refresh MCP config after a restore. */
    agentId?: string;
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
  /** Singleton full-window AI chat tab (the drawer's fullscreen mode). */
  const CHAT_ID = 'chat';
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
  /** Manual close of a live host/SFTP session waits for an explicit confirm. */
  let closeConfirmTabId = $state<string | null>(null);
  const closeConfirmTab = $derived(
    closeConfirmTabId ? tabs.find(t => t.id === closeConfirmTabId) ?? null : null,
  );
  /** The Note filter shows a short, newest-first list until asked for more. */
  let noteLimit = $state(10);

  $effect(() => {
    // A new filter or search term starts from the ten most recent notes again.
    void filter;
    void query;
    noteLimit = 10;
  });
  /** Pending risky-command approval from an agent or the built-in AI Chat. */
  interface McpConfirmRequest {
    id: string;
    session: string;
    command: string;
    reason: string;
    target: string;
  }
  let mcpConfirm = $state<McpConfirmRequest | null>(null);
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
  /** Image files open in a side preview instead of another tab. */
  let imagePreview = $state<{ sftpId: string; entry: SftpEntry } | null>(null);

  // ---------------------------------------------------------- persistence
  const TABS_KEY = 'rterm.tabs.v1';
  let tabsReady = false;

  const clampFraction = (value: unknown): number =>
    typeof value === 'number' && Number.isFinite(value)
      ? Math.min(0.8, Math.max(0.2, value))
      : 0.5;

  const lastId = (ids: string[]): string | null => ids[ids.length - 1] ?? null;

  /// Pane identity belongs to its terminals, not to its slot (left/right or
  /// top/bottom). Renaming a pane remounts every Terminal inside it and wipes
  /// the visible screen, which is exactly what a layout-only change must avoid.
  const otherPaneId = (id: 'a' | 'b'): 'a' | 'b' => (id === PANE_A ? PANE_B : PANE_A);

  const fixPaneActive = (pane: Pane): Pane => ({
    ...pane,
    active: pane.active && pane.tabs.includes(pane.active) ? pane.active : lastId(pane.tabs),
  });

  /// Collapse to a single pane when one of the two empties out; otherwise keep both.
  function normalizePanes(list: Pane[]): Pane[] {
    const nonEmpty = list.filter(p => p.tabs.length > 0);
    if (nonEmpty.length === 0) return [{ id: PANE_A, tabs: [], active: null }];
    // Keep the surviving pane's id — forcing PANE_A here would remount its
    // terminals when the other pane is closed.
    if (nonEmpty.length === 1) return [fixPaneActive(nonEmpty[0])];
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
    void syncMcpTarget(tabId);
  }

  /**
   * Keep the embedded bridge pointed at the SSH tab the user is looking at.
   * A new tab is not in Rust's session registry yet, so this call may fail;
   * `setStatus('connected')` retries once the handle is live.
   */
  async function syncMcpTarget(tabId: string | null) {
    if (!tabId) return;
    const tab = tabs.find(candidate => candidate.id === tabId);
    if (tab?.kind !== 'ssh' && tab?.kind !== 'adhoc') return;
    const pane = panes.find(candidate => candidate.id === focusedPaneId);
    if (pane?.active !== tabId) return;
    // A chat tab can be focused while the SSH tab stays the command target.
    try {
      await api.mcpSetTarget(tabId);
    } catch {
      /* session not ready (or already closed) — harmless */
    }
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
                  // Restored tabs keep only explicit prior consent; anything
                  // else defaults to no agent access (F-06).
                  allowAgent: (t.adhoc as { allowAgent?: unknown }).allowAgent === true,
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
            ...(typeof t.agentId === 'string' && t.kind === 'local' ? { agentId: t.agentId } : {}),
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
          ...(t.kind === 'local' && t.agentId ? { agentId: t.agentId } : {}),
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

  // Older sessions stored a literal "Note" title. Keep every open note tab in
  // sync with the first words of its body, including after a soft reload.
  $effect(() => {
    const list = notes();
    const stale = tabs.filter(tab => tab.title === 'Note' && typeof tab.hostId === 'string');
    if (!stale.length) return;
    for (const tab of stale) {
      const host = String(tab.hostId ?? '');
      const note = list.find(candidate =>
        candidate.id === host || candidate.id === host.replace(/^note-/, ''));
      if (note) syncNoteTabTitle(note);
    }
  });

  // All, Favorite and Note all search notes; Favorite narrows them to the
  // starred ones. Hosts keep their own `visible` list below.
  const noteList = $derived(
    notes()
      .filter(n => (filter === 'favorite' ? n.favorite : true))
      .filter(n => {
        const q = query.trim().toLowerCase();
        return !q || noteTitle(n.body).toLowerCase().includes(q) || n.body.toLowerCase().includes(q);
      })
      .sort((a, b) => Number(b.favorite) - Number(a.favorite) || (b.updated_at || '').localeCompare(a.updated_at || '')),
  );

  const visibleNotes = $derived(noteList.slice(0, noteLimit));

  const searchLabel = $derived(
    filter === 'note'
      ? 'Search notes'
      : filter === 'favorite'
        ? 'Search favorite hosts and notes'
        : 'Search hosts and notes',
  );

  const focusedPane = $derived(panes.find(p => p.id === focusedPaneId) ?? panes[0]);
  const focusedActiveTabId = $derived(focusedPane?.active ?? null);
  const activeTab = $derived(tabs.find(t => t.id === focusedActiveTabId) ?? null);
  const activeHostId = $derived(activeTab?.hostId ?? null);
  const activeNoteId = $derived(activeTab?.kind === 'note' ? activeTab.hostId : null);
  const tabById = $derived(new Map(tabs.map(t => [t.id, t] as const)));
  /** While the full tab exists the side drawer stays unmounted (one listener set). */
  const chatInTab = $derived(tabs.some(t => t.id === CHAT_ID));
  /** Minimizing the tab returns to an already-expanded right-side drawer. */
  let chatDrawerOpen = $state(false);
  /** Drives the shell's third grid column while the chat panel is expanded. */
  let chatPanelOpen = $state(false);
  /** Bumped when AI Chat asks Settings to jump to its BYOK section. */
  let settingsFocus = $state<{ section?: 'chat'; seq: number }>({ seq: 0 });
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

  /** Opens an SFTP file-manager tab for a saved host. */
  function openSftpHost(host: Host) {
    pickerOpen = false;
    const base = host.label || host.address;
    const count = tabs.filter(t => t.kind === 'sftp' && t.hostId === host.id).length;
    const tab: Tab = {
      id: crypto.randomUUID(),
      hostId: host.id,
      title: count > 0 ? `Files — ${base} ${count + 1}` : `Files — ${base}`,
      status: 'connecting',
      kind: 'sftp',
    };
    tabs = [...tabs, tab];
    addTabToPane(tab.id, lastFocusedOrFirst());
  }

  /** Quick-connect SFTP (agent auth only). */
  function openSftpAdhoc(target: AdhocTarget) {
    pickerOpen = false;
    const base = `${target.username}@${target.address}:${target.port}`;
    const count = tabs.filter(
      t => t.kind === 'sftp' && t.adhoc?.address === target.address && t.adhoc.username === target.username,
    ).length;
    const tab: Tab = {
      id: crypto.randomUUID(),
      hostId: '',
      title: count > 0 ? `Files — ${base} ${count + 1}` : `Files — ${base}`,
      status: 'connecting',
      kind: 'sftp',
      adhoc: target,
    };
    tabs = [...tabs, tab];
    addTabToPane(tab.id, lastFocusedOrFirst());
  }

  function openSftpFromTab(tab: Tab) {
    if (tab.kind === 'adhoc' && tab.adhoc) {
      openSftpAdhoc(tab.adhoc);
      return;
    }
    const host = hosts().find(h => h.id === tab.hostId);
    if (host) openSftpHost(host);
  }

  /** Text files keep opening in a tab; images open in the right-hand preview. */
  function openSftpFile(parent: Tab, entry: SftpEntry) {
    if (imageMimeForPath(entry.path)) {
      imagePreview = { sftpId: parent.id, entry };
      return;
    }

    const filePath = entry.path;
    const existing = tabs.find(t => t.kind === 'sftpfile' && t.filePath === filePath);
    if (existing) {
      activateTab(existing.id);
      return;
    }
    const tab: Tab = {
      id: crypto.randomUUID(),
      hostId: '',
      title: entry.name,
      status: 'connected',
      kind: 'sftpfile',
      // The backend SFTP session is keyed by the parent tab's id; sharing it
      // means no second auth and one connection, like VS Code's editor.
      sftpId: parent.id,
      filePath,
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
  // `noteTitle` lives in #lib/note-title so Rust agent replies can use the
  // exact same algorithm (see note-title-fixtures.json).

  function displayTabTitle(tab: Tab): string {
    if (tab.kind !== 'note') return tab.title;
    const note = notes().find(candidate => candidate.id === tab.hostId);
    return note ? noteTitle(note.body) : tab.title;
  }

  function syncNoteTabTitle(note: Note) {
    const title = noteTitle(note.body);
    const isNoteTab = (candidate: Tab) => candidate.kind === 'note' && candidate.hostId === note.id;
    const matches = tabs.filter(isNoteTab);
    if (!matches.length || matches.every(candidate => candidate.title === title)) return;
    tabs = tabs.map(candidate => (isNoteTab(candidate) ? { ...candidate, title } : candidate));
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
      syncNoteTabTitle(note);
      activateTab(id);
      return;
    }
    const tab: Tab = {
      id,
      hostId: note.id,
      title: noteTitle(note.body),
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
      syncNoteTabTitle({ ...note, body });
      void syncQuiet();
    } catch (error) {
      fail(error);
    }
  }

  async function toggleNoteFavorite(note: Note) {
    const favorite = !note.favorite;
    try {
      await saveNote({ ...note, favorite });
      void syncQuiet();
      notify(
        favorite
          ? `Note '${noteTitle(note.body)}' added to favorites`
          : `Note '${noteTitle(note.body)}' removed from favorites`,
      );
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

  /// Shared by the sidebar click and the AI chat `launch_agent` tool.
  function openAgentLaunch(launch: AgentLaunch, label: string, fallback?: AiAgent) {
    pickerOpen = false;
    const tab: Tab = {
      id: crypto.randomUUID(),
      hostId: '',
      title: label,
      status: 'connecting',
      kind: 'local',
      cwd: launch.cwd ?? '~/Desktop',
      command: launch.command ?? fallback?.command ?? label.toLowerCase(),
      env: launch.env ?? undefined,
      agentId: launch.agentId ?? fallback?.id,
    };
    tabs = [...tabs, tab];
    addTabToPane(tab.id, lastFocusedOrFirst());
  }

  /// Every detected CLI gets a private MCP config generated by the backend.
  /// The agent's own config files are never modified.
  async function openAgent(agent: AiAgent) {
    let launch: AgentLaunch | undefined;
    try {
      launch = await api.agentLaunch(agent.id);
    } catch (error) {
      console.warn('RuTerm MCP config unavailable; launching plain agent', error);
    }
    openAgentLaunch(
      launch ?? {
        agentId: agent.id,
        command: agent.command,
        cwd: '~/Desktop',
        env: undefined,
      },
      agent.label,
      agent,
    );
  }

  /** True while an agent tab is still starting up — drives the dock pulse. */
  function isAgentLaunching(agent: AiAgent): boolean {
    return tabs.some(
      t => t.kind === 'local' && t.agentId === agent.id && t.status === 'connecting',
    );
  }

  /// Send the user's answer back to the blocked MCP tool call.
  async function answerMcp(choice: 'deny' | 'allow' | 'always') {
    const request = mcpConfirm;
    mcpConfirm = null;
    if (!request) return;
    try {
      await api.mcpConfirm(request.id, choice);
    } catch (error) {
      console.warn('RuTerm could not deliver the MCP approval', error);
    }
  }

  // ------------------------------------------------------------------- clock
  let now = $state(new Date());
  let clockTimer: ReturnType<typeof setInterval> | null = null;
  const pad = (value: number) => String(value).padStart(2, '0');
  const clockText = $derived(
    `${pad(now.getHours())}:${pad(now.getMinutes())}:${pad(now.getSeconds())}`,
  );
  const clockMain = $derived(`${pad(now.getHours())}:${pad(now.getMinutes())}`);

  onDestroy(() => {
    if (clockTimer) clearInterval(clockTimer);
  });

  const QUICK_RE = /^\s*(?:ssh[ \t]+)?([A-Za-z0-9._-]+)@([A-Za-z0-9._-]+)(?::([0-9]{1,5}))?\s*$/;

  /** Backs the compact strip under the tab bar. Returns an error, or null on success. */
  function connectQuick(target: string, withAgent: boolean): string | null {
    const match = target.trim().match(QUICK_RE);
    if (!match) return 'Format: ssh username@host:port';
    const port = match[3] ? Number(match[3]) : 22;
    if (port < 1 || port > 65535) return 'Port must be between 1 and 65535';
    // F-06: quick-connect always uses the SSH agent. Require a deliberate
    // per-connection opt-in so injected script cannot silently turn the agent
    // into a signing oracle for an attacker-controlled host.
    if (!withAgent) return 'Tick the SSH agent checkbox to connect to a new host.';
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
        allowAgent: true,
      },
    };
    tabs = [...tabs, tab];
    addTabToPane(tab.id, lastFocusedOrFirst());
    return null;
  }

  // Compact strip directly under the tab bar.
  let barTarget = $state('');
  let barError = $state('');

  function barConnect(event: SubmitEvent) {
    event.preventDefault();
    // Pressing Connect is the deliberate opt-in for this one-off host.
    const error = connectQuick(barTarget, true);
    if (error) {
      barError = error;
      return;
    }
    barError = '';
    // Keep the address visible so the user can see which host was just opened.
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
    void syncMcpTarget(tabId);
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

  /** Open Settings and scroll straight to the AI chat configuration. */
  function openAiSettingsTab() {
    window.scrollTo(0, 0);
    settingsFocus = { section: 'chat', seq: settingsFocus.seq + 1 };
    openSettingsTab();
  }

  /** Promote the AI chat drawer into its own full-width tab. */
  function openChatTab() {
    chatPanelOpen = false;
    const existing = tabs.find(t => t.id === CHAT_ID);
    if (existing) {
      activateTab(CHAT_ID);
      return;
    }
    const tab: Tab = {
      id: CHAT_ID,
      hostId: '',
      title: 'RuTerm AI Chat',
      status: 'connected',
      kind: 'chat',
    };
    tabs = [...tabs, tab];
    addTabToPane(tab.id, lastFocusedOrFirst());
  }

  /** Collapse the full tab back into the right-side drawer without ending the chat. */
  function minimizeChatTab() {
    chatDrawerOpen = true;
    closeTab(CHAT_ID);
  }

  /** Close the chat entirely from the full tab (X button). */
  function closeChatTab() {
    chatDrawerOpen = false;
    chatPanelOpen = false;
    closeTab(CHAT_ID);
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
    const original = panes.find(p => p.tabs.includes(tab.id)) ?? panes[0];
    // The existing terminal keeps its pane id; only the new sibling gets the
    // other one. Array order supplies the visual left/right or top/bottom.
    const mine: Pane = {
      id: original.id,
      tabs: original.tabs.includes(tab.id) ? original.tabs : [...original.tabs, tab.id],
      active: tab.id,
    };
    const fresh: Pane = { id: otherPaneId(original.id), tabs: [sibling.id], active: sibling.id };
    applyPanes(toFirst ? [fresh, mine] : [mine, fresh]);
    focusedPaneId = mine.id;
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
      const original = panes[0];
      // Keep the dragged terminal in its current pane so its screen survives.
      // Move the *other* tabs to the sibling pane instead; the array order
      // still puts the dragged tab on the requested side/edge.
      const dropped: Pane = { id: original.id, tabs: [tabId], active: tabId };
      const others: Pane = {
        id: otherPaneId(original.id),
        tabs: rest,
        active: lastId(rest),
      };
      focusedPaneId = dropped.id;
      applyPanes(toFirst ? [dropped, others] : [others, dropped]);
    } else {
      const wantedIndex = toFirst ? 0 : 1;
      const currentIndex = panes.findIndex(p => p.tabs.includes(tabId));
      if (currentIndex !== -1 && currentIndex !== wantedIndex) {
        // A pure re-position (left/right/top/bottom) only swaps the visual
        // order of the panes. Moving the tab itself would remount its Terminal
        // and close the live SSH/local session attached to it.
        const swapped: Pane[] = [panes[1], panes[0]];
        applyPanes(swapped);
        focusedPaneId = swapped[1 - currentIndex].id;
      } else if (currentIndex !== -1) {
        focusedPaneId = panes[currentIndex].id;
      } else {
        const target = toFirst ? panes[0].id : panes[1].id;
        moveTabToPane(tabId, target, null);
      }
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
    if (status === 'connected' && (tab.kind === 'ssh' || tab.kind === 'adhoc')) {
      if (tab.kind === 'ssh') void maybeProbe(tab.hostId);
      void syncMcpTarget(id);
    }
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

  /// Only interactive host/SFTP sessions need a guard; page tabs close freely.
  function requestCloseTab(id: string) {
    const tab = tabs.find(t => t.id === id);
    if (tab?.kind === 'ssh' || tab?.kind === 'adhoc' || tab?.kind === 'sftp') {
      closeConfirmTabId = id;
      return;
    }
    closeTab(id);
  }

  function confirmCloseTab() {
    const id = closeConfirmTabId;
    closeConfirmTabId = null;
    if (id) closeTab(id);
  }

  function closeTab(id: string) {
    if (id === HOME_ID) return; // the Home tab is pinned
    // File-viewer tabs share the parent SFTP tab's backend session; when the
    // parent closes, the session is dropped, so the children must go too.
    const doomed = new Set<string>([id]);
    if (tabs.find(t => t.id === id)?.kind === 'sftp') {
      for (const t of tabs) {
        if (t.kind === 'sftpfile' && t.sftpId === id) doomed.add(t.id);
      }
      if (imagePreview?.sftpId === id) imagePreview = null;
    }
    for (const tabId of doomed) removeSnapshot(tabId);
    tabs = tabs.filter(t => !doomed.has(t.id));
    panes = panes.map(p => ({ ...p, tabs: p.tabs.filter(t => !doomed.has(t)) }));
    applyPanes(panes);
  }

  /// Trust on first use: pin the key so the next connect can verify it.
  async function onFingerprint(hostId: string, fingerprint: string) {
    const host = hosts().find(h => h.id === hostId);
    if (!host || host.host_key_fingerprint === fingerprint) return;
    await saveHost({ ...host, host_key_fingerprint: fingerprint });
  }

  onMount(() => {
    clockTimer = setInterval(() => (now = new Date()), 1000);

    // The MCP bridge runs in Rust; approval is always shown here, even when
    // the agent tab is focused and the SSH tab is in another pane.
    let unlistenConfirm: UnlistenFn | undefined;
    let unlistenHost: UnlistenFn | undefined;
    let unlistenOpenHost: UnlistenFn | undefined;
    let unlistenNote: UnlistenFn | undefined;
    let unlistenNoteOpen: UnlistenFn | undefined;
    let unlistenNoteClose: UnlistenFn | undefined;
    let unlistenTerminalClose: UnlistenFn | undefined;
    let unlistenNoteUpdated: UnlistenFn | undefined;
    let unlistenNoteDeleted: UnlistenFn | undefined;
    let unlistenAgent: UnlistenFn | undefined;
    let disposed = false;
    void listen<Omit<McpConfirmRequest, 'target'>>('ruterm://mcp-confirm', event => {
      if (disposed) return;
      const { id, session, command, reason } = event.payload;
      const tab = tabs.find(t => t.id === session);
      mcpConfirm = { id, session, command, reason, target: tab ? tab.title : session };
    })
      .then(fn => (disposed ? fn() : (unlistenConfirm = fn)))
      .catch(() => {});

    // The AI chat's `create_host` tool saves the host in Rust and asks us to
    // open it. Adopt the payload first so the sidebar already knows the host.
    void listen<{ id: string; payload: VaultPayload }>('rterm:host-created', event => {
      if (disposed) return;
      applyVaultPayload(event.payload.payload);
      const host = hosts().find(h => h.id === event.payload.id);
      if (!host) return;
      connect(host);
      notify(`Host '${host.label || host.address}' saved and opened`);
    })
      .then(fn => (disposed ? fn() : (unlistenHost = fn)))
      .catch(() => {});

    // Chat's `vps_open_host` asks us to open an existing saved host in a new
    // tab — same path as clicking it in the sidebar.
    void listen<{ id: string }>('rterm:open-host', event => {
      if (disposed) return;
      const host = hosts().find(candidate => candidate.id === event.payload.id);
      if (!host) return;
      connect(host);
      notify(`Opening '${host.label || host.address}' in a new tab`);
    })
      .then(fn => (disposed ? fn() : (unlistenOpenHost = fn)))
      .catch(() => {});

    // The AI chat's `create_note` tool writes directly to the vault.
    // Adopt the payload, then open the new note in a tab just like the + button.
    void listen<{ id: string; payload: VaultPayload }>('rterm:note-created', event => {
      if (disposed) return;
      applyVaultPayload(event.payload.payload);
      const note = notes().find(candidate => candidate.id === event.payload.id);
      if (!note) return;
      filter = 'note';
      openNote(note);
      notify(`Note '${noteTitle(note.body)}' saved and opened`);
    })
      .then(fn => (disposed ? fn() : (unlistenNote = fn)))
      .catch(() => {});

    // `open_note` (native chat) and `ruterm_open_note` (local agents) reuse the
    // same tab path as clicking a note, without writing anything to the vault.
    void listen<{ id: string }>('rterm:note-open', event => {
      if (disposed) return;
      const note = notes().find(candidate => candidate.id === event.payload.id);
      if (!note) return;
      filter = 'note';
      openNote(note);
      notify(`Note '${noteTitle(note.body)}' opened`);
    })
      .then(fn => (disposed ? fn() : (unlistenNoteOpen = fn)))
      .catch(() => {});

    // `close_note` / `ruterm_close_note` only close the tab; the note stays in
    // the vault and in the sidebar.
    void listen<{ id: string }>('rterm:close-note', event => {
      if (disposed) return;
      closeTab(`note-${event.payload.id}`);
      notify('Note tab closed');
    })
      .then(fn => (disposed ? fn() : (unlistenNoteClose = fn)))
      .catch(() => {});

    // `vps_close_terminal` / `ruterm_close_terminal` close the pinned SSH tab.
    // Rust clears the bridge target first; no local agent PTY is ever eligible.
    void listen<{ id: string }>('rterm:close-terminal', event => {
      if (disposed) return;
      closeTab(event.payload.id);
      notify('Terminal tab closed');
    })
      .then(fn => (disposed ? fn() : (unlistenTerminalClose = fn)))
      .catch(() => {});

    // Local AI agents can edit notes through the MCP bridge. Adopt the payload
    // and focus the note so the change is visible immediately, like chat's
    // `create_note`.
    void listen<{ id: string; payload: VaultPayload }>('rterm:note-updated', event => {
      if (disposed) return;
      applyVaultPayload(event.payload.payload);
      const note = notes().find(candidate => candidate.id === event.payload.id);
      if (!note) return;
      filter = 'note';
      openNote(note);
      notify(`Note '${noteTitle(note.body)}' updated`);
    })
      .then(fn => (disposed ? fn() : (unlistenNoteUpdated = fn)))
      .catch(() => {});

    // AI chat's `delete_note` soft-deletes in Rust. Adopt the payload and close
    // its tab so the UI never keeps showing a note that is gone from the vault.
    void listen<{ id: string; payload: VaultPayload }>('rterm:note-deleted', event => {
      if (disposed) return;
      applyVaultPayload(event.payload.payload);
      closeTab(`note-${event.payload.id}`);
      notify('Note deleted');
    })
      .then(fn => (disposed ? fn() : (unlistenNoteDeleted = fn)))
      .catch(() => {});

    // AI chat's `launch_agent` tool uses the same local-tab path as the dock.
    void listen<{ launch: AgentLaunch; label: string }>('rterm:agent-launch', event => {
      if (disposed) return;
      openAgentLaunch(event.payload.launch, event.payload.label);
      notify(`${event.payload.label} launched in a new local tab`);
    })
      .then(fn => (disposed ? fn() : (unlistenAgent = fn)))
      .catch(() => {});

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
    void syncMcpTarget(panes.find(p => p.id === focusedPaneId)?.active ?? null);
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
      disposed = true;
      unlistenConfirm?.();
      unlistenHost?.();
      unlistenOpenHost?.();
      unlistenNote?.();
      unlistenNoteOpen?.();
      unlistenNoteClose?.();
      unlistenTerminalClose?.();
      unlistenNoteUpdated?.();
      unlistenNoteDeleted?.();
      unlistenAgent?.();
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

{#snippet noteRow(note: Note)}
  <div
    class="host note"
    class:active={activeNoteId === note.id}
    role="button"
    tabindex="0"
    title={collapsed ? noteTitle(note.body) : note.body.slice(0, 120)}
    onclick={() => openNote(note)}
    onkeydown={event => event.key === 'Enter' && openNote(note)}
  >
    {#if !collapsed}
      <span class="note-icon" aria-hidden="true"><Icon name="file" size={17} /></span>
      <span class="meta">
        <span class="name">{noteTitle(note.body)}</span>
        <span class="sub">{noteTime(note)}</span>
      </span>
      <button
        class="more star-toggle"
        class:fav={note.favorite}
        type="button"
        title={note.favorite ? 'Remove from favorites' : 'Add to favorites'}
        aria-label={note.favorite
          ? `Remove '${noteTitle(note.body)}' from favorites`
          : `Add '${noteTitle(note.body)}' to favorites`}
        aria-pressed={note.favorite}
        onclick={event => {
          event.stopPropagation();
          void toggleNoteFavorite(note);
        }}
      >
        <span aria-hidden="true">{note.favorite ? '★' : '☆'}</span>
      </button>
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
      <span class="note-icon" aria-hidden="true"><Icon name="file" size={19} /></span>
      {#if note.favorite}<span class="rail-star" aria-hidden="true">★</span>{/if}
    {/if}
  </div>
{/snippet}

{#snippet noteRows()}
  {#if visibleNotes.length}
    {#if !collapsed}
      <div class="list-heading">{filter === 'favorite' ? 'Favorite notes' : 'Notes'}</div>
    {/if}
    {#each visibleNotes as note (note.id)}
      {@render noteRow(note)}
    {/each}
    {#if !collapsed && noteList.length > noteLimit}
      <button class="load-more" type="button" onclick={() => (noteLimit += 10)}>
        Load more notes ({noteList.length - noteLimit})
      </button>
    {/if}
  {/if}
{/snippet}

{#snippet hostRows()}
  {#if visible.length}
    {#if !collapsed}
      <div class="list-heading">{filter === 'favorite' ? 'Favorite hosts' : 'Hosts'}</div>
    {/if}
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
            class="more sftp"
            type="button"
            title="Files (SFTP)"
            aria-label="Browse files on {host.label || host.address} over SFTP"
            onclick={event => {
              event.stopPropagation();
              openSftpHost(host);
            }}
          >
            <Icon name="folder" size={16} />
          </button>
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
    {/each}
  {/if}
{/snippet}

<div
  class="shell"
  class:collapsed
  class:chat-open={chatPanelOpen && !chatInTab}
  class:backdrop={background.mode !== 'none'}
  style={backgroundStyle()}
>
  <aside>
    <header>
      {#if collapsed}
        <button
          class="rail-expand"
          type="button"
          title="Expand sidebar"
          aria-label="Expand sidebar"
          onclick={toggleSidebar}
        >
          <Icon name="chevron-right" size={18} />
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
        <input autocapitalize="off" autocorrect="off" spellcheck="false" bind:value={query} placeholder={searchLabel} aria-label={searchLabel} />
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
        {#each visibleNotes as note (note.id)}
          {@render noteRow(note)}
        {:else}
          {#if !collapsed}
            <div class="empty">
              <p class="muted">{query ? 'No notes match your search.' : 'No notes yet.'}</p>
              <button class="btn sm" onclick={() => void openNewNote()}>New note</button>
            </div>
          {/if}
        {/each}

        {#if !collapsed && noteList.length > noteLimit}
          <button class="load-more" type="button" onclick={() => (noteLimit += 10)}>
            Load more notes ({noteList.length - noteLimit})
          </button>
        {/if}

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
        {@render hostRows()}
        {@render noteRows()}

        {#if !collapsed && !visibleNotes.length && !visible.length}
          <div class="empty">
            <p class="muted">
              {query
                ? 'No hosts or notes match your search.'
                : filter === 'favorite'
                  ? 'No favorites yet.'
                  : 'Nothing here yet.'}
            </p>
            <div class="empty-actions">
              <button class="btn sm" onclick={startBlank}>Add host</button>
              <button class="btn sm" onclick={() => void openNewNote()}>New note</button>
            </div>
          </div>
        {/if}

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

    {#if aiAgents.length > 0}
      <div class="side-ai" role="toolbar" aria-label="AI Agent ( Available on this device )">
        {#each aiAgents as agent (agent.id)}
          <button
            class="side-ai-btn"
            class:launching={isAgentLaunching(agent)}
            type="button"
            title={isAgentLaunching(agent) ? `Launching ${agent.label}…` : `${agent.label} — opens in ~/Desktop`}
            aria-label="Open {agent.label} in ~/Desktop"
            aria-busy={isAgentLaunching(agent)}
            onclick={() => openAgent(agent)}
          >
            <AiAgentIcon id={agent.id} size={collapsed ? 20 : 19} />
          </button>
        {/each}
      </div>
    {/if}

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
          {#if account}
            <button
              class="icon-btn"
              title="Change account"
              aria-label="Change account"
              disabled={app.busy}
              onclick={() => void changeAccount()}
            >
              <Icon name="refresh" size={15} stroke={2.3} />
            </button>
          {/if}
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
    <div class="workspace-wrap" class:with-preview={imagePreview !== null}>
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
                          ? `${displayTabTitle(tab)} — drag to reorder, double-click for another session`
                          : `${displayTabTitle(tab)} — drag to reorder`}
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
                      {:else if tab.kind === 'chat'}
                        <span class="tabicon page"><Icon name="spark" size={13} /></span>
                      {:else if tab.kind === 'adhoc'}
                        <span class="tabicon local"><Icon name="terminal" size={13} /></span>
                      {:else if tab.kind === 'sftp'}
                        <span class="tabicon page"><Icon name="folder" size={13} /></span>
                      {:else if tab.kind === 'sftpfile'}
                        <span class="tabicon page"><Icon name="file-text" size={13} /></span>
                      {:else if tab.kind === 'note'}
                        <span class="tabicon page"><Icon name="file" size={13} /></span>
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
                      <span class="tabname">{displayTabTitle(tab)}</span>
                      {#if tab.kind !== 'home'}
                        <button
                          class="close"
                          type="button"
                          title="Close tab"
                          aria-label="Close tab"
                          onclick={event => {
                            event.stopPropagation();
                            requestCloseTab(tab.id);
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

            <form class="quickbar" onsubmit={barConnect}>
              <input
                class="qc-input mono"
                bind:value={barTarget}
                placeholder="user@host:port"
                aria-label="Quick SSH connect"
                autocapitalize="off"
                autocomplete="off"
                autocorrect="off"
                spellcheck="false"
              />
              {#if barError}
                <span class="qc-error" role="alert">{barError}</span>
              {/if}
              <button class="btn primary qc-button" type="submit">
                Connect
                <Icon name="chevron-right" size={14} />
              </button>
            </form>

            <div class="panes">
              {#each pane.tabs as id (id)}
                {@const tab = tabById.get(id)}
                {#if tab}
                  <div class="pane" class:hidden={pane.active !== id}>
                    {#if tab.kind === 'home'}
                      <div class="empty-main home-panel">
                        <div class="hero" class:frosted={background.mode === 'image'} style={frostStyle()}>
                          <div class="clock-view" role="timer" aria-label="Current time">
                            <div class="digital-clock mono">{clockText}</div>
                            <div class="clock-brand">RuTerm <span>|</span> SSH Client</div>
                          </div>
                          <div class="hero-actions">
                            <button class="btn primary" onclick={startBlank}>
                              <Icon name="plus" size={17} />
                              New host
                            </button>
                            <button class="btn" onclick={openLocal} title="Open a shell on this device">
                              <Icon name="terminal" size={16} />
                              Local terminal
                            </button>
                            <button class="btn" onclick={openWhatsNewTab} title="Open release notes">
                              <Icon name="spark" size={16} />
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
                    {:else if tab.kind === 'chat'}
                      <AiChatDrawer
                        variant="tab"
                        onminimize={minimizeChatTab}
                        onclose={closeChatTab}
                        onopensettings={openAiSettingsTab}
                      />
                    {:else if tab.kind === 'settings'}
                      <Settings
                        focus={settingsFocus}
                        onclose={() => closeTab(tab.id)}
                        onopeninfo={openInfoTab}
                        onopenwhatsnew={openWhatsNewTab}
                      />
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
                    {:else if tab.kind === 'sftp'}
                      <SftpPanel
                        sftpId={tab.id}
                        hostId={tab.hostId}
                        adhoc={tab.adhoc}
                        onstatus={status => setStatus(tab.id, status)}
                        onopenfile={entry => openSftpFile(tab, entry)}
                      />
                    {:else if tab.kind === 'sftpfile'}
                      <SftpFileView sftpId={tab.sftpId ?? ''} path={tab.filePath ?? ''} />
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
                        env={tab.env}
                        agentId={tab.agentId}
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

      {#if imagePreview}
        <div class="preview-divider" aria-hidden="true"></div>
        <div class="preview-slot">
          {#key `${imagePreview.sftpId}:${imagePreview.entry.path}`}
            <ImagePreview
              sftpId={imagePreview.sftpId}
              entry={imagePreview.entry}
              onclose={() => (imagePreview = null)}
            />
          {/key}
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
            <span class="action-icon note"><Icon name="file" size={15} /></span>
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
            <div
              class="picker-row"
              role="button"
              tabindex="0"
              onclick={() => connect(host)}
              onkeydown={event => event.key === 'Enter' && connect(host)}
            >
              <HostAvatar host={host} size={26} />
              <span class="meta">
                <span class="name">{host.label || host.address}</span>
                <span class="sub mono">{host.username}@{host.address}:{host.port}</span>
              </span>
              {#if openCount(host.id) > 0}
                <span class="badge">{openCount(host.id)}×</span>
              {/if}
              <button
                class="icon-btn picker-sftp"
                type="button"
                title="Files (SFTP)"
                aria-label="Browse files on {host.label || host.address} over SFTP"
                onclick={event => {
                  event.stopPropagation();
                  openSftpHost(host);
                }}
              >
                <Icon name="folder" size={16} />
              </button>
            </div>
          {:else}
            {#each pickable.slice(0, 5) as host (host.id)}
              <div
                class="picker-row"
                role="button"
                tabindex="0"
                onclick={() => connect(host)}
                onkeydown={event => event.key === 'Enter' && connect(host)}
              >
                <HostAvatar host={host} size={26} />
                <span class="meta">
                  <span class="name">{host.label || host.address}</span>
                  <span class="sub mono">{host.username}@{host.address}:{host.port}</span>
                </span>
                {#if openCount(host.id) > 0}
                  <span class="badge">{openCount(host.id)}×</span>
                {/if}
                <button
                  class="icon-btn picker-sftp"
                  type="button"
                  title="Files (SFTP)"
                  aria-label="Browse files on {host.label || host.address} over SFTP"
                  onclick={event => {
                    event.stopPropagation();
                    openSftpHost(host);
                  }}
                >
                  <Icon name="folder" size={16} />
                </button>
              </div>
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
          {#if menuTab.kind === 'ssh' || menuTab.kind === 'adhoc'}
          <button type="button" role="menuitem" title="Browse files on this host over SFTP" onclick={() => { closeTabMenu(); openSftpFromTab(menuTab); }}>
            <Icon name="folder" size={15} />
            Files (SFTP)
          </button>
          {/if}
          {#if menuTab.kind === 'note'}
            <button type="button" role="menuitem" class="danger" title="Delete this note" onclick={() => { closeTabMenu(); noteToDelete = menuTab.hostId ? notes().find(n => n.id === menuTab.hostId) ?? null : null; }}>
              <Icon name="trash" size={15} />
              Delete note
            </button>
          {/if}
          <button type="button" role="menuitem" class="danger" title="Close this session" onclick={() => { closeTabMenu(); requestCloseTab(menuTab.id); }}>
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

  {#if noteToDelete}
    <ConfirmDialog
      title="Delete this note?"
      confirmLabel="Delete note"
      oncancel={() => (noteToDelete = null)}
      onconfirm={() => void confirmDeleteNote()}
    >
      <p>The note <strong>{noteTitle(noteToDelete.body)}</strong> will be removed from your vault — on this device and everywhere it syncs.</p>
    </ConfirmDialog>
  {/if}

  {#if closeConfirmTab}
    <ConfirmDialog
      title="Close this vault tab?"
      confirmLabel="Close tab"
      oncancel={() => (closeConfirmTabId = null)}
      onconfirm={confirmCloseTab}
    >
      <p>
        Closing <strong>{closeConfirmTab.title}</strong> will end its active session. You can
        reconnect from the sidebar anytime.
      </p>
      {#if closeConfirmTab.kind === 'sftp'}
        <p class="mcp-hint">File viewers opened from this SFTP session will close too.</p>
      {/if}
    </ConfirmDialog>
  {/if}

  {#if mcpConfirm}
    <ConfirmDialog
      title="Run this command on the VPS?"
      cancelLabel="Deny"
      confirmLabel="Allow"
      alwaysLabel="Always allow"
      tone="primary"
      oncancel={() => void answerMcp('deny')}
      onconfirm={() => void answerMcp('allow')}
      onalways={() => void answerMcp('always')}
    >
      <p>
        An AI assistant wants to <strong>{mcpConfirm.reason}</strong> in <strong>{mcpConfirm.target}</strong>.
        The command will be typed into that SSH tab.
      </p>
      <pre class="mcp-command">{mcpConfirm.command}</pre>
      <p class="mcp-hint">“Always allow” remembers this exact command for the current RuTerm session.</p>
    </ConfirmDialog>
  {/if}

  {#if !chatInTab}
    <AiChatDrawer
      startOpen={chatDrawerOpen}
      onfullscreen={openChatTab}
      onopenchange={value => (chatPanelOpen = value)}
      onopensettings={openAiSettingsTab}
    />
  {/if}

  <TrafficLights />
</div>

<style>
  .mcp-command {
    margin: 12px 0 0;
    padding: 10px 12px;
    max-height: 130px;
    overflow: auto;
    border: 1px solid var(--line);
    border-radius: 9px;
    background: rgba(0, 0, 0, 0.22);
    color: var(--text);
    font: 12px/1.55 ui-monospace, 'SF Mono', Menlo, monospace;
    text-align: left;
    white-space: pre-wrap;
    word-break: break-word;
  }
  .mcp-hint {
    margin-top: 9px !important;
    font-size: 12px !important;
    color: var(--text-secondary) !important;
  }

  .shell {
    position: relative;
    height: 100%;
    display: grid;
    grid-template-columns: 280px minmax(0, 1fr);
    overflow: hidden;
    transition: grid-template-columns 0.18s ease;
  }
  .shell.collapsed {
    grid-template-columns: 84px minmax(0, 1fr);
  }
  /* The chat panel is a real third column of the page, not an overlay. */
  .shell.chat-open {
    grid-template-columns: 280px minmax(0, 1fr) min(420px, 34vw);
  }
  .shell.collapsed.chat-open {
    grid-template-columns: 84px minmax(0, 1fr) min(420px, 34vw);
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
  /* Expand affordance sits a little lower so it clears the native traffic
     lights instead of crowding them. */
  .collapsed aside header {
    justify-content: center;
    padding: 16px 0 12px;
  }
  .rail-expand {
    display: grid;
    place-items: center;
    width: 34px;
    height: 34px;
    padding: 0;
    border-radius: 50%;
    border: 1px solid var(--accent-dim);
    background: var(--accent-dim);
    color: var(--accent);
    cursor: pointer;
    transition:
      background 0.15s ease,
      border-color 0.15s ease;
  }
  .rail-expand:hover {
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
  .load-more {
    display: block;
    width: calc(100% - 8px);
    margin: 6px 4px 0;
    padding: 7px 9px;
    border: 1px dashed var(--line);
    border-radius: 9px;
    background: transparent;
    color: var(--text-tertiary);
    font-size: 11.5px;
    cursor: pointer;
  }
  .load-more:hover {
    color: var(--text);
    border-color: var(--accent);
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
  .note-icon {
    display: grid;
    place-items: center;
    flex: 0 0 auto;
    color: var(--text-tertiary);
  }
  .host.active .note-icon {
    color: var(--accent);
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
  .star-toggle {
    font-size: 14px;
    line-height: 1;
  }
  .star-toggle.fav {
    opacity: 1;
    color: var(--warn);
  }
  .star-toggle.fav:hover {
    color: var(--text);
  }
  .rail-star {
    position: absolute;
    top: 2px;
    right: 3px;
    color: var(--warn);
    font-size: 9px;
  }
  .list-heading {
    padding: 10px 8px 4px;
    color: var(--text-tertiary);
    font-size: 10.5px;
    font-weight: 700;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }
  .empty-actions {
    display: flex;
    justify-content: center;
    gap: 8px;
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
    gap: 24px;
    width: 100%;
    max-width: 700px;
    text-align: center;
  }
  /* Only wallpaper mode gets a subtle frosted surface; no card otherwise. */
  .hero.frosted {
    padding: 28px 32px;
    border: 1px solid var(--line);
    border-radius: 18px;
    background: color-mix(in srgb, var(--panel) var(--frost-alpha, 45%), transparent);
    backdrop-filter: blur(var(--frost-blur, 10px)) saturate(150%);
    -webkit-backdrop-filter: blur(var(--frost-blur, 10px)) saturate(150%);
  }
  @supports not ((backdrop-filter: blur(1px)) or (-webkit-backdrop-filter: blur(1px))) {
    .hero.frosted {
      background: color-mix(in srgb, var(--panel) 92%, transparent);
    }
  }
  .clock-view {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
    user-select: none;
  }
  .digital-clock {
    font-size: clamp(42px, 6vw, 72px);
    font-weight: 300;
    line-height: 1;
    letter-spacing: -0.04em;
    color: color-mix(in srgb, var(--text) 85%, var(--bg));
    font-variant-numeric: tabular-nums;
  }
  .clock-brand {
    font-size: 14px;
    font-weight: 600;
    letter-spacing: 0.14em;
    color: color-mix(in srgb, var(--text) 65%, var(--bg));
  }
  .clock-brand span {
    margin: 0 6px;
    color: var(--accent);
  }
  /* Shared by the compact quick-connect strip under the tab bar. */
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
  .qc-error {
    margin: 0;
    font-size: 12px;
    color: var(--bad);
  }
  /* ------------------------------------- AI agents, icon-only dock */
  .side-ai {
    flex: 0 0 auto;
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    max-height: 116px;
    overflow-y: auto;
    padding: 8px 10px 10px;
    border-top: 1px solid var(--line);
  }
  .collapsed .side-ai {
    flex-direction: column;
    flex-wrap: nowrap;
    align-items: center;
    gap: 5px;
    padding: 8px;
  }
  .side-ai-btn {
    display: grid;
    place-items: center;
    width: 32px;
    height: 32px;
    padding: 0;
    border: 1px solid var(--line);
    border-radius: 9px;
    background: var(--panel-hover);
    cursor: pointer;
    transition: background 0.14s ease, border-color 0.14s ease, transform 0.14s ease;
  }
  .side-ai-btn :global(.ai-icon) {
    display: block;
    border-radius: 7px;
  }
  .side-ai-btn:hover,
  .side-ai-btn:focus-visible {
    border-color: var(--accent);
    background: color-mix(in srgb, var(--accent) 16%, var(--panel-hover));
    transform: translateY(-1px);
  }
  .side-ai-btn.launching {
    border-color: var(--accent);
  }
  .side-ai-btn.launching :global(.ai-icon) {
    animation: aiLaunchPulse 1.15s ease-in-out infinite;
  }
  .collapsed .side-ai-btn {
    width: 34px;
    height: 34px;
  }
  .side-ai-btn:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
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
    .side-ai-btn.launching :global(.ai-icon) {
      animation: none;
    }
  }

  .hero-actions {
    display: flex;
    flex-wrap: wrap;
    justify-content: center;
    gap: 10px;
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
    border: 1px solid var(--line);
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
    /* Blue reaches the very top edge; the native traffic lights sit over it. */
    background: color-mix(in srgb, var(--accent) 18%, var(--tabbar));
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

  /* Compact quick-connect strip directly under the tab bar. */
  .quickbar {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: 0 0 48px;
    height: 48px;
    min-width: 0;
    padding: 0 14px;
    border-bottom: 1px solid var(--line);
    background: var(--panel);
  }
  .quickbar .qc-input {
    flex: 1;
    height: 34px;
    min-width: 0;
    padding: 0 12px;
    border-radius: 8px;
    font-size: 13px;
  }
  .quickbar .qc-input::placeholder {
    font-size: 12.5px;
  }
  .quickbar .qc-error {
    flex: 0 1 auto;
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .quickbar .qc-button {
    margin-left: auto;
    height: 34px;
    padding: 0 16px;
    border-radius: 8px;
    font-size: 13px;
  }


  /* ---------- split workspace ---------- */
  .workspace-wrap {
    flex: 1;
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: row;
  }
  .workspace-wrap.with-preview .workspace {
    flex: 1 1 56%;
  }
  .preview-divider {
    flex: 0 0 1px;
    background: var(--line);
  }
  .preview-slot {
    flex: 1 1 44%;
    min-width: 320px;
    max-width: 60%;
    min-height: 0;
    display: flex;
    background: var(--panel);
  }

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
    min-width: 72px;
    max-width: 240px;
    height: 36px;
    padding: 0 6px 0 12px;
    border: 0;
    border-radius: 11px 11px 0 0;
    background: var(--tabbar);
    color: var(--text-secondary);
    font-size: 13px;
    cursor: pointer;
    white-space: nowrap;
    /* Normal size while there is room; shrink gradually once the strip fills. */
    flex: 0 1 auto;
    margin-bottom: 0;
    position: relative;
    transition:
      background 0.14s ease,
      color 0.14s ease,
      min-width 0.16s ease;
  }
  .tab:hover,
  .tab:focus-visible {
    flex: 0 0 auto;
    z-index: 3;
    color: var(--text);
    background: var(--panel-hover);
  }
  .tab.active {
    flex: 0 0 auto;
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
    min-width: 0;
    flex: 0 1 auto;
    overflow: hidden;
    text-overflow: ellipsis;
    font-weight: 500;
  }
  .close {
    display: none;
    place-items: center;
    width: 19px;
    height: 19px;
    padding: 0;
    flex: 0 0 auto;
    border: 0;
    border-radius: 5px;
    background: transparent;
    color: var(--text-tertiary);
    cursor: pointer;
  }
  .tab:hover .close,
  .tab:focus-visible .close,
  .tab.active .close {
    display: grid;
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
  .picker-sftp {
    margin-left: auto;
    flex: 0 0 auto;
    width: 30px;
    height: 30px;
    border-radius: 7px;
    opacity: 0;
    transition: opacity 0.12s ease;
  }
  .picker-row:hover .picker-sftp,
  .picker-row:focus-within .picker-sftp {
    opacity: 1;
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
