<script lang="ts">
  import { onMount } from 'svelte';
  import { marked } from 'marked';
  import DOMPurify from 'dompurify';
  import Icon from './Icon.svelte';
  import { loadNews, loadUnreadIds, markAllRead, type NewsEntry } from '$lib/news.svelte';

  let { onclose }: { onclose?: () => void } = $props();

  const REPO_RAW = 'https://raw.githubusercontent.com/dnysaz/ruterm-app/main';

  let entries = $state<NewsEntry[]>([]);
  let source = $state<'github' | 'cache' | 'bundle' | 'empty' | 'loading'>('loading');
  let unreadIds = $state(new Set<string>());
  let refreshing = $state(false);

  function prettyDate(iso: string): string {
    const parsed = new Date(`${iso}T00:00:00Z`);
    if (Number.isNaN(parsed.getTime())) return iso;
    return parsed.toLocaleDateString('en-US', {
      year: 'numeric',
      month: 'short',
      day: 'numeric',
      timeZone: 'UTC',
    });
  }

  function renderMarkdown(markdown: string): string {
    let html = marked.parse(markdown, { gfm: true, breaks: true }) as string;
    // Relative links/images in notes point at the repo — send them to GitHub.
    html = html.replace(/src="(?!https?:|data:)([^"]+)"/g, `src="${REPO_RAW}/$1"`);
    html = html.replace(/href="(?!https?:|#|mailto:)([^"]+)"/g, `href="https://github.com/dnysaz/ruterm-app/blob/main/$1"`);
    if (typeof DOMPurify?.sanitize === 'function') return DOMPurify.sanitize(html);
    // Sanitizer unavailable — show the escaped source rather than raw HTML.
    return html.replace(/</g, '&lt;').replace(/>/g, '&gt;');
  }

  async function refresh(force: boolean) {
    refreshing = true;
    try {
      const feed = await loadNews(force);
      if (unreadIds.size === 0 && source === 'loading') {
        // First paint: capture what was unread, then clear the badge.
        unreadIds = loadUnreadIds(feed.entries);
      }
      entries = feed.entries;
      source = feed.source;
      // Opening the tab clears the unread badge.
      markAllRead(feed.entries);
    } finally {
      refreshing = false;
    }
  }

  onMount(() => {
    window.getSelection()?.removeAllRanges();
    void refresh(false);
  });
</script>

<div class="whatsnew-page">
  <header class="page-head">
    <span class="page-icon"><Icon name="spark" size={19} /></span>
    <div class="titles">
      <h1>What's New</h1>
      <p class="muted sub">Release notes — straight from the repo, newest first.</p>
    </div>
    <span class="spacer"></span>
    <span class="source" class:fresh={source === 'github'}>
      {#if source === 'loading'}
        Checking…
      {:else if source === 'github'}
        from GitHub
      {:else if source === 'cache'}
        offline · cached copy
      {:else if source === 'bundle'}
        offline · bundled in app
      {:else}
        offline
      {/if}
    </span>
    <button
      class="icon-btn"
      title="Check for new notes"
      aria-label="Check for new notes"
      disabled={refreshing}
      onclick={() => void refresh(true)}
    >
      <Icon name="sync" size={15} />
    </button>
    {#if onclose}
      <button class="icon-btn" title="Close" aria-label="Close" onclick={onclose}>
        <Icon name="close" size={16} />
      </button>
    {/if}
  </header>

  <div class="scroll">
    <div class="column">
      {#if source === 'loading'}
        <p class="muted empty">Loading release notes…</p>
      {:else if entries.length === 0}
        <p class="muted empty">
          Nothing here yet. Notes land in <span class="mono">News/</span> in the repo —
          write one, push, and it shows up here.
        </p>
      {:else}
        {#each entries as entry (entry.file)}
          <article class="card">
            <header class="entry-head">
              <span class="date">{prettyDate(entry.date)}</span>
              {#if unreadIds.has(entry.file)}<span class="unread-dot" title="Unread"></span>{/if}
              <span class="spacer"></span>
              <span class="file mono" title={entry.file}>{entry.file}</span>
            </header>
            <div class="md">{@html renderMarkdown(entry.markdown ?? '')}</div>
          </article>
        {/each}
        <p class="foot muted tiny">
          Bundled with the app for offline reading · refreshes from
          <span class="mono">github.com/dnysaz/ruterm-app</span> when online.
          Unread dots clear when this tab opens.
        </p>
      {/if}
    </div>
  </div>
</div>

<style>
  .whatsnew-page {
    height: 100%;
    display: flex;
    flex-direction: column;
    background: var(--panel);
    overflow: hidden;
  }

  .page-head {
    flex: 0 0 auto;
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 16px 26px;
    border-bottom: 1px solid var(--line);
    background: var(--panel);
  }
  .page-icon {
    display: grid;
    place-items: center;
    width: 38px;
    height: 38px;
    border-radius: 10px;
    border: 1px solid var(--accent-line);
    color: var(--accent);
    background: var(--accent-dim);
    flex: 0 0 auto;
  }
  .titles h1 {
    margin: 0;
    font-size: 20.5px;
    font-weight: 600;
    letter-spacing: -0.01em;
  }
  .sub {
    margin: 3px 0 0;
    font-size: 14px;
  }
  .source {
    font-size: 12px;
    padding: 4px 10px;
    border-radius: 999px;
    border: 1px solid var(--line);
    color: var(--dim);
    user-select: none;
    white-space: nowrap;
  }
  .source.fresh {
    border-color: var(--accent-line);
    color: var(--accent-strong);
  }

  .scroll {
    flex: 1;
    overflow-y: auto;
    padding: 22px 26px 44px;
  }
  .column {
    width: 100%;
    max-width: 48rem;
    margin: 0 auto;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }
  .empty {
    padding: 40px 8px;
    text-align: center;
    line-height: 1.7;
  }

  .card {
    border: 1px solid var(--line);
    border-radius: 14px;
    background: transparent;
    padding: 16px 20px 14px;
  }
  .entry-head {
    display: flex;
    align-items: center;
    gap: 12px;
    margin-bottom: 10px;
    padding-bottom: 9px;
    border-bottom: 1px solid var(--line);
    user-select: none;
  }
  .date {
    font-size: 12.5px;
    font-weight: 600;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--accent-strong);
  }
  .unread-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--accent);
    flex: 0 0 auto;
  }
  .file {
    font-size: 11.5px;
    color: var(--dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 40%;
  }

  .md {
    font-size: 15px;
    line-height: 1.65;
    max-width: 66ch;
  }
  .md :global(h1) {
    margin: 2px 0 10px;
    font-size: 18px;
    font-weight: 650;
    letter-spacing: -0.015em;
    line-height: 1.3;
  }
  .md :global(h2) {
    margin: 16px 0 8px;
    font-size: 15px;
    font-weight: 600;
    letter-spacing: 0.05em;
    text-transform: uppercase;
    color: var(--dim);
  }
  .md :global(h3) {
    margin: 14px 0 6px;
    font-size: 15px;
    font-weight: 600;
  }
  .md :global(p) {
    margin: 0 0 10px;
  }
  .md :global(p:last-child) {
    margin-bottom: 0;
  }
  .md :global(ul),
  .md :global(ol) {
    margin: 0 0 10px;
    padding-left: 1.3em;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .md :global(a) {
    color: var(--accent-strong);
    text-decoration: underline;
    text-underline-offset: 3px;
    text-decoration-color: var(--accent-line);
  }
  .md :global(strong) {
    color: var(--text);
  }
  .md :global(code) {
    font-family: var(--term);
    font-size: 0.88em;
    background: var(--surface);
    border: 1px solid var(--line);
    border-radius: 5px;
    padding: 1px 5px;
  }
  .md :global(pre) {
    background: #161311;
    border: 1px solid var(--line-strong);
    border-radius: 12px;
    padding: 12px 14px;
    overflow-x: auto;
    margin: 0 0 10px;
  }
  .md :global(pre code) {
    background: none;
    border: 0;
    padding: 0;
    font-size: 13px;
    line-height: 1.7;
    color: #e8e2d8;
  }
  .md :global(img) {
    max-width: 100%;
    border-radius: 10px;
  }

  .foot {
    text-align: center;
    line-height: 1.7;
    padding: 4px 12px 0;
  }
</style>
