<script lang="ts">
  import { onDestroy, onMount, tick } from 'svelte';
  import {
    api,
    errorText,
    forgetSftpToken,
    rememberSftpToken,
    type SessionStatus,
    type SftpDir,
    type SftpEntry,
  } from '#lib/api';
  import Icon from './Icon.svelte';
  import ConfirmDialog from './ConfirmDialog.svelte';

  interface AdhocTarget {
    username: string;
    address: string;
    port: number;
    allowAgent: boolean;
  }

  let {
    sftpId,
    hostId = '',
    adhoc = undefined,
    onstatus,
    onopenfile,
  }: {
    /** Tab id — also the key of the backend SFTP session. */
    sftpId: string;
    /** Saved vault host id, or empty for a quick-connect target. */
    hostId?: string;
    adhoc?: AdhocTarget;
    onstatus?: (status: SessionStatus) => void;
    /** Double-clicking (or long-press opening) a file hands it to the editor tab. */
    onopenfile?: (entry: SftpEntry) => void;
  } = $props();

  type Phase = 'connecting' | 'error' | 'ready';

  let phase = $state<Phase>('connecting');
  let error = $state('');
  let dir = $state<SftpDir | null>(null);
  let loading = $state(false);
  /** Number of background transfers in flight (upload/download). */
  let busy = $state(0);
  let selected = $state<string | null>(null);
  /** Which inline "name" row is being created: folder or file. */
  let creating = $state<'folder' | 'file' | null>(null);
  let newName = $state('');
  let renaming = $state<string | null>(null);
  let renameDraft = $state('');
  let toDelete = $state<SftpEntry | null>(null);
  /** Transient message shown in the footer toast pill. */
  let toast = $state<string | null>(null);
  let toastTimer: ReturnType<typeof setTimeout> | null = null;
  let dropHover = $state(false);
  let info = $state<{ user: string; address: string; port: number } | null>(null);
  /** Rust-issued capability for this SFTP session; required on every call. */
  let sftpToken = $state('');
  /** Dot files are hidden by default (like Finder); this toggles them. */
  let showHidden = $state(false);

  const pathSegments = $derived.by(() => {
    const p = dir?.path ?? '/';
    const parts = p.split('/').filter(Boolean);
    const crumbs: { name: string; path: string }[] = [{ name: '/', path: '/' }];
    let acc = '';
    for (const part of parts) {
      acc += `/${part}`;
      crumbs.push({ name: part, path: acc });
    }
    return crumbs;
  });

  const realEntries = $derived(
    (dir?.entries ?? []).filter(entry => entry.name !== '.' && entry.name !== '..'),
  );
  const visibleEntries = $derived.by(() => {
    return showHidden
      ? realEntries
      : realEntries.filter(entry => !entry.name.startsWith('.'));
  });
  const hiddenCount = $derived(realEntries.length - visibleEntries.length);

  const parentPath = $derived.by(() => {
    const p = dir?.path ?? '/';
    if (p === '/' || p === '') return null;
    const trimmed = p.replace(/\/+$/, '');
    const i = trimmed.lastIndexOf('/');
    if (i <= 0) return '/';
    return trimmed.slice(0, i);
  });

  function showToast(message: string) {
    toast = message;
    if (toastTimer) clearTimeout(toastTimer);
    toastTimer = setTimeout(() => (toast = null), 4200);
  }

  function joinPath(dirPath: string, name: string): string {
    return dirPath.endsWith('/') ? `${dirPath}${name}` : `${dirPath}/${name}`;
  }

  function safeEntryName(name: string): string | null {
    const trimmed = name.trim();
    if (!trimmed || trimmed === '.' || trimmed === '..' || /[/\\\0]/.test(trimmed)) return null;
    return trimmed;
  }

  function baseName(p: string): string {
    const trimmed = p.replace(/\/+$/, '');
    return trimmed.slice(trimmed.lastIndexOf('/') + 1);
  }

  function setStatus(status: SessionStatus) {
    onstatus?.(status);
  }

  async function connect() {
    phase = 'connecting';
    error = '';
    try {
      const opened = adhoc
        ? await api.sftpOpenAdhoc(sftpId, adhoc.username, adhoc.address, adhoc.port, adhoc.allowAgent === true)
        : await api.sftpOpenHost(sftpId, hostId);
      info = { user: opened.user, address: opened.address, port: opened.port };
      sftpToken = opened.token;
      rememberSftpToken(sftpId, opened.token);
      phase = 'ready';
      setStatus('connected');
      await refresh(opened.home);
    } catch (e) {
      error = errorText(e);
      phase = 'error';
      setStatus('error');
    }
  }

  async function refresh(path?: string) {
    const target = path ?? dir?.path ?? '/';
    loading = true;
    try {
      dir = await api.sftpList(sftpId, sftpToken, target);
      selected = null;
    } catch (e) {
      showToast(errorText(e));
    } finally {
      loading = false;
    }
  }

  function enterDir(entry: SftpEntry) {
    if (!entry.is_dir) return;
    void refresh(entry.path);
  }

  function openEntry(entry: SftpEntry) {
    if (entry.is_dir) {
      enterDir(entry);
    } else {
      onopenfile?.(entry);
    }
  }

  // ------------------------------------------------------------- actions
  async function uploadFiles() {
    try {
      const handles = await api.sftpPickUploads();
      if (!handles.length) return;
      busy += handles.length;
      let failed = 0;
      for (const localToken of handles) {
        try {
          await api.sftpUpload(sftpId, sftpToken, localToken, dir?.path ?? '/');
        } catch {
          failed += 1;
        } finally {
          busy -= 1;
        }
      }
      if (failed > 0) showToast(`${failed} file${failed > 1 ? 's' : ''} failed to upload`);
      await refresh();
    } catch (e) {
      busy = 0;
      showToast(errorText(e));
    }
  }

  async function downloadEntry(entry: SftpEntry) {
    try {
      const localToken = await api.sftpPickDownload(entry.name);
      busy += 1;
      try {
        await api.sftpDownload(sftpId, sftpToken, entry.path, localToken);
      } finally {
        busy -= 1;
      }
    } catch (e) {
      showToast(errorText(e));
    }
  }

  function startCreate(kind: 'folder' | 'file') {
    creating = kind;
    newName = '';
    void tick().then(() => {
      (document.querySelector('.sftp-new-input') as HTMLInputElement | null)?.focus();
    });
  }

  async function commitCreate() {
    const kind = creating;
    const name = newName.trim();
    creating = null;
    newName = '';
    if (!name || /[/\\]/.test(name)) {
      if (name) showToast('A name cannot contain / or \\');
      return;
    }
    const remotePath = joinPath(dir?.path ?? '/', name);
    try {
      if (kind === 'file') {
        await api.sftpCreateFile(sftpId, sftpToken, remotePath);
        await refresh();
        // A brand-new file is useless to leave sitting there — open it.
        onopenfile?.({ name, path: remotePath, is_dir: false, is_symlink: false, size: 0, mtime: Math.floor(Date.now() / 1000), permissions: null });
        return;
      }
      await api.sftpMkdir(sftpId, sftpToken, remotePath);
      await refresh();
    } catch (e) {
      showToast(errorText(e));
    }
  }

  function startRename(entry: SftpEntry) {
    renaming = entry.path;
    renameDraft = entry.name;
    void tick().then(() => {
      (
        document.querySelector('.sftp-rename-input') as HTMLInputElement | null
      )?.focus();
    });
  }

  async function commitRename(entry: SftpEntry) {
    const draft = renameDraft.trim();
    const target = renaming;
    renaming = null;
    renameDraft = '';
    if (!target || !draft || draft === entry.name) return;
    if (/[/\\]/.test(draft)) {
      showToast('A name cannot contain / or \\');
      return;
    }
    const parent = target.slice(0, target.length - entry.name.length);
    try {
      await api.sftpRename(sftpId, sftpToken, target, joinPath(parent, draft));
      await refresh();
    } catch (e) {
      showToast(errorText(e));
    }
  }

  async function confirmDelete() {
    const entry = toDelete;
    toDelete = null;
    if (!entry) return;
    try {
      await api.sftpRemove(sftpId, sftpToken, entry.path, entry.is_dir);
      await refresh();
    } catch (e) {
      showToast(errorText(e));
    }
  }

  // ------------------------------------------------------------- dnd upload
  function bytesToBase64(bytes: Uint8Array): string {
    let binary = '';
    const chunk = 0x8000;
    for (let i = 0; i < bytes.length; i += chunk) {
      binary += String.fromCharCode(...bytes.subarray(i, i + chunk));
    }
    return btoa(binary);
  }

  async function dropFiles(event: DragEvent) {
    event.preventDefault();
    dropHover = false;
    const files = Array.from(event.dataTransfer?.files ?? []);
    if (files.length === 0) return;
    const target = dir?.path ?? '/';
    busy += files.length;
    let failed = 0;
    for (const file of files) {
      const name = safeEntryName(file.name);
      if (!name) {
        failed += 1;
        continue;
      }
      try {
        const buf = new Uint8Array(await file.arrayBuffer());
        await api.sftpUploadBytes(sftpId, sftpToken, joinPath(target, name), bytesToBase64(buf));
      } catch {
        failed += 1;
      } finally {
        busy -= 1;
      }
    }
    if (failed > 0) showToast(`${failed} file${failed > 1 ? 's' : ''} failed to upload`);
    await refresh();
  }

  function dragOver(event: DragEvent) {
    if (event.dataTransfer?.types?.includes('Files')) {
      event.preventDefault();
      event.dataTransfer.dropEffect = 'copy';
      dropHover = true;
    }
  }

  function dragLeave(event: DragEvent) {
    if (event.currentTarget === event.target) dropHover = false;
  }

  onMount(() => {
    void connect();
  });

  onDestroy(() => {
    if (toastTimer) clearTimeout(toastTimer);
    void api.sftpClose(sftpId, sftpToken).catch(() => {});
    forgetSftpToken(sftpId);
  });

  // ------------------------------------------------------------- format
  function fmtSize(size: number | null, isDir: boolean): string {
    if (isDir) return '—';
    if (size === null || size === undefined) return '—';
    if (size < 1024) return `${size} B`;
    const units = ['KB', 'MB', 'GB', 'TB'];
    let value = size / 1024;
    let i = 0;
    while (value >= 1024 && i < units.length - 1) {
      value /= 1024;
      i += 1;
    }
    return `${value >= 100 ? Math.round(value) : value.toFixed(1)} ${units[i]}`;
  }

  function fmtTime(mtime: number | null): string {
    if (!mtime) return '—';
    const d = new Date(mtime * 1000);
    const now = new Date();
    const sameDay =
      d.getFullYear() === now.getFullYear() &&
      d.getMonth() === now.getMonth() &&
      d.getDate() === now.getDate();
    if (sameDay) {
      return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
    }
    const opts: Intl.DateTimeFormatOptions = { month: 'short', day: 'numeric' };
    if (d.getFullYear() !== now.getFullYear()) opts.year = 'numeric';
    return d.toLocaleDateString([], opts);
  }
</script>

<div class="sftp">
  {#if phase === 'connecting'}
    <div class="state">
      <div class="spinner" aria-hidden="true"></div>
      <p class="state-title">Connecting…</p>
      <p class="state-sub">Opening an SFTP session</p>
    </div>
  {:else if phase === 'error'}
    <div class="state">
      <p class="state-title">Couldn’t open SFTP</p>
      <p class="state-sub">{error}</p>
      {#if adhoc}
        <p class="state-sub tiny">Quick-connect SFTP uses your SSH agent — passwords are never kept. Saved hosts work with any auth method.</p>
      {/if}
      <button class="btn primary" type="button" onclick={connect}>Retry</button>
    </div>
  {:else}
    <header class="bar">
      <button
        class="icon-btn"
        type="button"
        title="Up one level"
        aria-label="Up one level"
        disabled={!parentPath}
        onclick={() => parentPath && void refresh(parentPath)}
      >
        <Icon name="arrow-up" size={17} />
      </button>
      <nav class="crumbs" aria-label="Path">
        {#each pathSegments as crumb, i (crumb.path)}
          {#if i > 0}<span class="sep" aria-hidden="true">/</span>{/if}
          <button
            class="crumb"
            class:current={i === pathSegments.length - 1}
            type="button"
            onclick={() => void refresh(crumb.path)}
          >{crumb.name === '/' ? '/' : crumb.name}</button>
        {/each}
      </nav>
      <span class="flex-spacer"></span>
      <button
        class="icon-btn toggle"
        class:active={showHidden}
        type="button"
        title={showHidden ? 'Hide hidden files' : 'Show hidden files'}
        aria-pressed={showHidden}
        aria-label={showHidden ? 'Hide hidden files' : 'Show hidden files'}
        onclick={() => (showHidden = !showHidden)}
      >
        <Icon name={showHidden ? 'eye' : 'eye-off'} size={17} />
      </button>
      <button
        class="icon-btn"
        type="button"
        title="Refresh"
        aria-label="Refresh"
        onclick={() => void refresh()}
      >
        <Icon name="refresh" size={17} />
      </button>
      <button class="btn sm" type="button" onclick={uploadFiles} title="Upload files (or drop them here)">
        <Icon name="upload" size={15} />
        Upload
      </button>
      <button class="btn sm" type="button" onclick={() => startCreate('file')} title="Create a new file (any extension)">
        <Icon name="file" size={15} />
        New file
      </button>
      <button class="btn sm" type="button" onclick={() => startCreate('folder')} title="New folder">
        <Icon name="folder" size={15} />
        New folder
      </button>
    </header>

    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="table-wrap"
      class:drop-target={dropHover}
      ondragover={dragOver}
      ondragleave={dragLeave}
      ondrop={dropFiles}
    >
      <div class="thead" aria-hidden="true">
        <span class="col-name">Name</span>
        <span class="col-size">Size</span>
        <span class="col-time">Modified</span>
        <span class="col-perms">Permissions</span>
        <span class="col-actions"></span>
      </div>

      {#if creating}
        <div class="row creating">
          <Icon name={creating === 'file' ? 'file' : 'folder'} size={17} stroke={1.8} />
          <!-- svelte-ignore a11y_autofocus -->
          <input
            class="sftp-new-input"
            type="text"
            placeholder={creating === 'file' ? 'File name (any extension, e.g. index.html)' : 'Folder name'}
            bind:value={newName}
            onblur={commitCreate}
            onkeydown={event => {
              if (event.key === 'Enter') void commitCreate();
              else if (event.key === 'Escape') {
                creating = null;
                newName = '';
              }
            }}
          />
        </div>
      {/if}

      {#each visibleEntries as entry (entry.path)}
        {#if renaming === entry.path}
          <div class="row renaming">
            <Icon name={entry.is_dir ? 'folder' : 'file'} size={17} stroke={1.8} />
            <!-- svelte-ignore a11y_autofocus -->
            <input
              class="sftp-rename-input"
              type="text"
              bind:value={renameDraft}
              onblur={() => void commitRename(entry)}
              onkeydown={event => {
                if (event.key === 'Enter') void commitRename(entry);
                else if (event.key === 'Escape') {
                  renaming = null;
                  renameDraft = '';
                }
              }}
            />
          </div>
        {:else}
          <div
            class="row"
            class:selected={selected === entry.path}
            class:dir={entry.is_dir}
            role="button"
            tabindex="0"
            ondblclick={() => openEntry(entry)}
            onclick={() => (selected = entry.path)}
            onkeydown={event => {
              if (event.key === 'Enter') openEntry(entry);
            }}
            title={entry.path}
          >
            <span class="col-name">
              <Icon name={entry.is_dir ? 'folder' : 'file'} size={17} stroke={1.8} />
              <span class="name">{entry.name}</span>
              {#if entry.is_symlink}<span class="symlink tiny">→</span>{/if}
            </span>
            <span class="col-size mono">{fmtSize(entry.size, entry.is_dir)}</span>
            <span class="col-time">{fmtTime(entry.mtime)}</span>
            <span class="col-perms mono">{entry.permissions ?? ''}</span>
            <span class="col-actions">
              {#if !entry.is_dir}
                <button
                  class="icon-btn xs"
                  type="button"
                  title="Download"
                  aria-label={`Download ${entry.name}`}
                  onclick={event => {
                    event.stopPropagation();
                    void downloadEntry(entry);
                  }}
                >
                  <Icon name="download" size={15} />
                </button>
              {/if}
              <button
                class="icon-btn xs"
                type="button"
                title="Rename"
                aria-label={`Rename ${entry.name}`}
                onclick={event => {
                  event.stopPropagation();
                  startRename(entry);
                }}
              >
                <Icon name="edit" size={15} />
              </button>
              <button
                class="icon-btn xs danger"
                type="button"
                title="Delete"
                aria-label={`Delete ${entry.name}`}
                onclick={event => {
                  event.stopPropagation();
                  toDelete = entry;
                }}
              >
                <Icon name="trash" size={15} />
              </button>
            </span>
          </div>
        {/if}
      {/each}

      {#if dir && visibleEntries.length === 0 && realEntries.length > 0 && !creating}
        <div class="empty">
          <Icon name="eye-off" size={28} stroke={1.4} />
          <p>{realEntries.length} hidden item{realEntries.length === 1 ? '' : 's'}</p>
          <button class="btn sm" type="button" onclick={() => (showHidden = true)}>
            <Icon name="eye" size={15} />
            Show hidden files
          </button>
        </div>
      {:else if dir && realEntries.length === 0 && !creating && !loading}
        <div class="empty">
          <Icon name="folder" size={28} stroke={1.4} />
          <p>This folder is empty</p>
          <p class="tiny">Drop files here to upload them</p>
        </div>
      {/if}
      {#if loading}
        <div class="loading-veil" aria-hidden="true"></div>
      {/if}
    </div>

    <footer class="statusbar">
      <span class="who mono">{info ? `${info.user}@${info.address}:${info.port}` : ''}</span>
      <span class="path mono">{dir?.path ?? ''}</span>
      <span class="flex-spacer"></span>
      {#if busy > 0}
        <span class="busy"><span class="spinner xs" aria-hidden="true"></span>{busy} transfer{busy > 1 ? 's' : ''}</span>
      {/if}
      {#if hiddenCount > 0}
        <span class="count">{hiddenCount} hidden</span>
      {/if}
      <span class="count">{visibleEntries.length} item{visibleEntries.length === 1 ? '' : 's'}</span>
    </footer>

    {#if toast}
      <div class="toast" role="status">{toast}</div>
    {/if}
  {/if}

  {#if toDelete}
    <ConfirmDialog
      title={toDelete.is_dir ? 'Delete this folder and everything in it?' : 'Delete this file?'}
      confirmLabel="Delete"
      onconfirm={() => void confirmDelete()}
      oncancel={() => (toDelete = null)}
    >
      <p class="confirm-path mono">{toDelete.path}</p>
      {#if toDelete.is_dir}
        <p>Everything inside this folder will be permanently removed from the server.</p>
      {/if}
    </ConfirmDialog>
  {/if}
</div>

<style>
  .sftp {
    position: relative;
    height: 100%;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    background: var(--panel);
    color: var(--text);
    font-family: var(--sans);
  }

  .flex-spacer {
    flex: 1 1 auto;
  }

  /* ---------------- full-panel states ---------------- */
  .state {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 10px;
    padding: 40px;
    text-align: center;
  }
  .state-title {
    margin: 4px 0 0;
    font-size: 15px;
    font-weight: 600;
  }
  .state-sub {
    margin: 0;
    max-width: 420px;
    color: var(--text-secondary);
    font-size: 13px;
    line-height: 1.45;
    word-break: break-word;
  }
  .tiny {
    font-size: 11.5px;
    color: var(--text-tertiary);
  }

  .spinner {
    width: 26px;
    height: 26px;
    border-radius: 50%;
    border: 2px solid var(--line);
    border-top-color: var(--accent);
    animation: spin 0.9s linear infinite;
  }
  .spinner.xs {
    width: 12px;
    height: 12px;
    border-width: 1.5px;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .spinner {
      animation: none;
    }
  }

  /* ---------------- toolbar ---------------- */
  .bar {
    display: flex;
    align-items: center;
    gap: 6px;
    height: 46px;
    padding: 0 12px;
    border-bottom: 1px solid var(--line);
    background: var(--bg-elevated);
    flex: 0 0 auto;
  }

  .crumbs {
    display: flex;
    align-items: center;
    gap: 2px;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    font-size: 13px;
  }
  .crumbs .sep {
    color: var(--text-tertiary);
    user-select: none;
  }
  .crumb {
    border: 0;
    background: transparent;
    color: var(--text-secondary);
    font: inherit;
    font-size: 13px;
    padding: 4px 6px;
    border-radius: 6px;
    cursor: pointer;
    max-width: 220px;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .crumb:hover {
    color: var(--text);
    background: var(--panel-hover);
  }
  .crumb.current {
    color: var(--text);
    font-weight: 600;
  }

  /* ---------------- file table ---------------- */
  .table-wrap {
    position: relative;
    flex: 1;
    overflow-y: auto;
    outline: none;
  }
  .table-wrap.drop-target {
    box-shadow: inset 0 0 0 2px var(--accent);
    background: var(--accent-dim);
  }

  .thead {
    position: sticky;
    top: 0;
    z-index: 2;
    display: grid;
    grid-template-columns: minmax(0, 1fr) 90px 132px 112px 88px;
    gap: 12px;
    padding: 7px 16px;
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.04em;
    text-transform: uppercase;
    color: var(--text-tertiary);
    background: var(--panel);
    border-bottom: 1px solid var(--line);
  }
  .col-size,
  .col-time,
  .col-perms,
  .col-actions {
    text-align: left;
  }

  .row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 90px 132px 112px 88px;
    gap: 12px;
    align-items: center;
    width: 100%;
    padding: 5px 16px;
    border: 0;
    background: transparent;
    color: inherit;
    font: inherit;
    text-align: left;
    cursor: default;
    border-bottom: 1px solid var(--line);
    user-select: none;
  }
  .row:hover {
    background: var(--panel-hover);
  }
  .row.selected {
    background: var(--accent-dim);
  }
  .row:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }

  .col-name {
    display: flex;
    align-items: center;
    gap: 9px;
    min-width: 0;
    color: var(--accent);
  }
  .row.dir .col-name {
    color: var(--accent);
  }
  .row:not(.dir) .col-name {
    color: var(--text-tertiary);
  }
  .col-name .name {
    color: var(--text);
    font-size: 13px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .row.dir .col-name .name {
    font-weight: 500;
  }
  .symlink {
    color: var(--text-tertiary);
  }

  .col-size {
    font-size: 12px;
    color: var(--text-secondary);
  }
  .col-time {
    font-size: 12px;
    color: var(--text-secondary);
  }
  .col-perms {
    font-size: 11px;
    color: var(--text-tertiary);
    letter-spacing: 0.02em;
  }
  .mono {
    font-family: var(--mono);
  }

  .col-actions {
    display: flex;
    align-items: center;
    gap: 2px;
    opacity: 0;
    transition: opacity 0.12s ease;
  }
  .row:hover .col-actions,
  .row:focus-within .col-actions {
    opacity: 1;
  }
  .icon-btn.toggle {
    color: var(--text-tertiary);
  }
  .icon-btn.toggle.active {
    color: var(--accent);
    background: var(--accent-dim);
  }

  .icon-btn.xs {
    width: 26px;
    height: 26px;
    border-radius: 6px;
  }
  .icon-btn.xs.danger:hover {
    color: var(--bad);
  }

  .row.creating,
  .row.renaming {
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 5px 16px;
    color: var(--accent);
  }
  .sftp-new-input,
  .sftp-rename-input {
    flex: 1;
    height: 26px;
    padding: 0 8px;
    border: 1px solid var(--accent);
    border-radius: 6px;
    background: var(--bg-elevated);
    color: var(--text);
    font: inherit;
    font-size: 13px;
    outline: none;
  }

  .empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    padding: 56px 20px;
    text-align: center;
    color: var(--text-tertiary);
  }
  .empty p {
    margin: 0;
    font-size: 13px;
    color: var(--text-secondary);
  }
  .empty .tiny {
    color: var(--text-tertiary);
  }

  .loading-veil {
    position: absolute;
    inset: 0;
    background: transparent;
    pointer-events: none;
  }

  /* ---------------- status bar ---------------- */
  .statusbar {
    flex: 0 0 auto;
    display: flex;
    align-items: center;
    gap: 12px;
    height: 30px;
    padding: 0 12px;
    border-top: 1px solid var(--line);
    background: var(--bg-elevated);
    font-size: 11px;
    color: var(--text-tertiary);
    white-space: nowrap;
    overflow: hidden;
  }
  .statusbar .who {
    color: var(--text-secondary);
    font-size: 10.5px;
  }
  .statusbar .path {
    overflow: hidden;
    text-overflow: ellipsis;
    font-size: 10.5px;
  }
  .busy {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    color: var(--accent);
  }

  /* ---------------- toast ---------------- */
  .toast {
    position: absolute;
    left: 50%;
    bottom: 40px;
    transform: translateX(-50%);
    max-width: min(480px, 80%);
    padding: 8px 14px;
    border-radius: 8px;
    background: var(--bg-elevated);
    border: 1px solid var(--line-hover);
    color: var(--text);
    font-size: 12.5px;
    z-index: 5;
  }

  .confirm-path {
    margin: 0;
    font-size: 12px;
    color: var(--text-secondary);
    word-break: break-all;
  }
</style>
