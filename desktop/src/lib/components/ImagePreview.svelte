<script lang="ts">
  import { onMount } from 'svelte';
  import { api, errorText, getSftpToken, type SftpEntry } from '#lib/api';
  import { imageMimeForPath } from '#lib/images';
  import Icon from './Icon.svelte';

  let {
    sftpId,
    entry,
    onclose,
  }: {
    sftpId: string;
    entry: SftpEntry;
    onclose: () => void;
  } = $props();

  type Phase = 'loading' | 'ready' | 'error';

  let phase = $state<Phase>('loading');
  let error = $state('');
  let src = $state<string | null>(null);
  let width = $state(0);
  let height = $state(0);
  let size = $state(0);
  let copied = $state(false);
  let copyTimer: ReturnType<typeof setTimeout> | null = null;

  const mime = $derived(imageMimeForPath(entry.path) ?? 'application/octet-stream');
  const format = $derived(mime.slice(mime.indexOf('/') + 1).toUpperCase());
  const fileName = $derived(entry.name || entry.path.split('/').pop() || entry.path);

  function formatBytes(n: number): string {
    if (!n) return '—';
    if (n < 1024) return `${n} B`;
    const units = ['KB', 'MB', 'GB'];
    let value = n / 1024;
    let i = 0;
    while (value >= 1024 && i < units.length - 1) {
      value /= 1024;
      i += 1;
    }
    return `${value >= 100 ? Math.round(value) : value.toFixed(1)} ${units[i]}`;
  }

  function formatTime(seconds: number | null): string {
    if (!seconds) return 'Not reported';
    return new Date(seconds * 1000).toLocaleString(undefined, {
      year: 'numeric',
      month: 'short',
      day: '2-digit',
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit',
    });
  }

  async function load() {
    phase = 'loading';
    error = '';
    try {
      const image = await api.sftpReadImage(sftpId, getSftpToken(sftpId), entry.path);
      src = `data:${mime};base64,${image.data}`;
      size = entry.size ?? image.size;
      phase = 'ready';
    } catch (e) {
      error = errorText(e);
      phase = 'error';
    }
  }

  function onImageLoad(event: Event) {
    const img = event.currentTarget as HTMLImageElement;
    width = img.naturalWidth;
    height = img.naturalHeight;
  }

  async function copyPath() {
    try {
      await navigator.clipboard.writeText(entry.path);
      copied = true;
      if (copyTimer) clearTimeout(copyTimer);
      copyTimer = setTimeout(() => (copied = false), 1800);
    } catch {
      copied = false;
    }
  }

  async function download() {
    try {
      const token = await api.sftpPickDownload(fileName);
      await api.sftpDownload(sftpId, getSftpToken(sftpId), entry.path, token);
    } catch {
      /* the server may refuse downloads; the preview stays usable */
    }
  }

  onMount(() => {
    void load();
    return () => {
      if (copyTimer) clearTimeout(copyTimer);
    };
  });
</script>

<div class="image-preview">
  <header class="bar">
    <div class="heading">
      <span class="eyebrow">Image preview</span>
      <strong title={entry.path}>{fileName}</strong>
    </div>
    <div class="actions">
      <button
        class="icon-btn"
        type="button"
        title="Download a copy"
        aria-label="Download a copy"
        onclick={() => void download()}
      >
        <Icon name="download" size={16} />
      </button>
      <button
        class="icon-btn"
        type="button"
        title="Close preview"
        aria-label="Close preview"
        onclick={onclose}
      >
        <Icon name="close" size={16} />
      </button>
    </div>
  </header>

  <div class="viewer">
    {#if phase === 'loading'}
      <div class="state">
        <div class="spinner" aria-hidden="true"></div>
        <p>Loading image…</p>
      </div>
    {:else if phase === 'error'}
      <div class="state">
        <Icon name="alert" size={28} stroke={1.4} />
        <p class="state-title">Couldn’t load this image</p>
        <p class="state-sub">{error}</p>
        <button class="btn sm" type="button" onclick={() => void load()}>Retry</button>
      </div>
    {:else if src}
      <img class="preview" {src} alt={fileName} draggable="false" onload={onImageLoad} />
    {/if}
  </div>

  <section class="details" aria-label="File details">
    <h2>File details</h2>
    <dl>
      <div>
        <dt>Name</dt>
        <dd class="mono">{fileName}</dd>
      </div>
      <div>
        <dt>Format</dt>
        <dd>{format} <span class="muted">· {mime}</span></dd>
      </div>
      <div>
        <dt>Dimensions</dt>
        <dd>{width && height ? `${width} × ${height} px` : '—'}</dd>
      </div>
      <div>
        <dt>Size</dt>
        <dd>{formatBytes(size)}</dd>
      </div>
      <div>
        <dt>Saved at</dt>
        <dd>{formatTime(entry.mtime)}</dd>
      </div>
      <div>
        <dt>Permissions</dt>
        <dd class="mono">{entry.permissions ?? '—'}</dd>
      </div>
      <div class="wide">
        <dt>Full path</dt>
        <dd class="path mono">
          <span>{entry.path}</span>
          <button
            class="icon-btn"
            type="button"
            title={copied ? 'Copied' : 'Copy full path'}
            aria-label="Copy full path"
            onclick={() => void copyPath()}
          >
            <Icon name={copied ? 'check' : 'copy'} size={14} />
          </button>
        </dd>
      </div>
    </dl>
  </section>
</div>

<style>
  .image-preview {
    flex: 1;
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    background: var(--panel);
    color: var(--text);
    font-family: var(--sans);
  }

  .mono {
    font-family: var(--mono);
  }
  .muted {
    color: var(--text-tertiary);
  }

  .bar {
    flex: 0 0 auto;
    display: flex;
    align-items: flex-end;
    gap: 12px;
    height: 68px;
    padding: 30px 12px 8px 16px;
    border-bottom: 1px solid var(--line);
    background: var(--bg-elevated);
  }
  .heading {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
    flex: 1;
  }
  .heading .eyebrow {
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 0.09em;
    text-transform: uppercase;
    color: var(--text-tertiary);
  }
  .heading strong {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 13px;
    font-weight: 600;
  }
  .actions {
    display: flex;
    gap: 2px;
    flex: 0 0 auto;
  }

  .viewer {
    flex: 1 1 auto;
    min-height: 120px;
    overflow: auto;
    display: grid;
    place-items: center;
    padding: 16px;
    background-color: var(--bg);
    background-image: linear-gradient(45deg, var(--panel-hover) 25%, transparent 25%),
      linear-gradient(-45deg, var(--panel-hover) 25%, transparent 25%),
      linear-gradient(45deg, transparent 75%, var(--panel-hover) 75%),
      linear-gradient(-45deg, transparent 75%, var(--panel-hover) 75%);
    background-size: 16px 16px;
    background-position: 0 0, 0 8px, 8px -8px, -8px 0;
  }
  .preview {
    max-width: 100%;
    max-height: 100%;
    object-fit: contain;
    user-select: none;
    -webkit-user-drag: none;
  }

  .state {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
    text-align: center;
    color: var(--text-secondary);
    font-size: 13px;
  }
  .state-title {
    margin: 0;
    font-size: 14px;
    font-weight: 600;
    color: var(--text);
  }
  .state-sub {
    margin: 0;
    max-width: 34ch;
    line-height: 1.5;
    word-break: break-word;
  }
  .spinner {
    width: 26px;
    height: 26px;
    border-radius: 50%;
    border: 2px solid var(--line);
    border-top-color: var(--accent);
    animation: spin 0.9s linear infinite;
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

  .details {
    flex: 0 0 auto;
    max-height: 42%;
    overflow-y: auto;
    padding: 14px 16px 18px;
    border-top: 1px solid var(--line);
    background: var(--bg-elevated);
  }
  .details h2 {
    margin: 0 0 12px;
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 0.09em;
    text-transform: uppercase;
    color: var(--text-tertiary);
  }
  .details dl {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 12px 18px;
    margin: 0;
  }
  .details dl > div {
    min-width: 0;
  }
  .details dl > div.wide {
    grid-column: 1 / -1;
  }
  .details dt {
    margin-bottom: 3px;
    font-size: 10.5px;
    font-weight: 600;
    letter-spacing: 0.02em;
    color: var(--text-tertiary);
  }
  .details dd {
    margin: 0;
    font-size: 12px;
    line-height: 1.45;
    color: var(--text);
    overflow-wrap: anywhere;
  }
  .details dd.path {
    display: flex;
    align-items: flex-start;
    gap: 6px;
    padding: 7px 8px;
    border: 1px solid var(--line);
    border-radius: 8px;
    background: var(--bg);
    font-size: 11px;
    color: var(--text-secondary);
  }
  .details dd.path span {
    flex: 1;
    min-width: 0;
    overflow-wrap: anywhere;
  }
</style>
