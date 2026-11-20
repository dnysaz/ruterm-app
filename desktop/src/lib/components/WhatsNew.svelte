<script lang="ts">
  import { onMount } from 'svelte';
  import { renderSafeMarkdown } from '#lib/markdown';
  import Icon from './Icon.svelte';
  import { loadNews, loadUnreadIds, markAllRead, type NewsEntry } from '#lib/news.svelte';

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
    // Relative links/images in release notes point at the repo — send them to GitHub.
    return renderSafeMarkdown(markdown, {
      linkBase: 'https://github.com/dnysaz/ruterm-app/blob/main/',
      imageBase: `${REPO_RAW}/`,
    });
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
          <article class="entry">
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
    background: var(--bg);
    overflow: hidden;
  }

  .page-head {
    flex: 0 0 auto;
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 20px 28px 14px;
    background: var(--bg);
  }
  .page-icon {
    display: grid;
    place-items: center;
    width: 34px;
    height: 34px;
    border-radius: 10px;
    color: var(--accent);
    background: var(--accent-dim);
    flex: 0 0 auto;
  }
  .titles h1 {
    margin: 0;
    font-size: 20px;
    font-weight: 600;
    letter-spacing: -0.02em;
  }
  .sub {
    margin: 3px 0 0;
    font-size: 13px;
    color: var(--text-secondary);
  }
  .source {
    font-size: 11.5px;
    padding: 4px 10px;
    border-radius: 999px;
    background: color-mix(in srgb, var(--text) 8%, transparent);
    color: var(--text-tertiary);
    user-select: none;
    white-space: nowrap;
  }
  .source.fresh {
    color: var(--accent);
  }

  .scroll {
    flex: 1;
    overflow-y: auto;
    padding: 4px 28px 48px;
  }
  .column {
    width: 100%;
    max-width: 680px;
    margin: 0 auto;
    display: flex;
    flex-direction: column;
    gap: 36px;
  }
  .empty {
    padding: 48px 8px;
    text-align: center;
    line-height: 1.7;
    color: var(--text-secondary);
  }

  .entry-head {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-bottom: 10px;
    user-select: none;
  }
  .date {
    font-size: 11.5px;
    font-weight: 600;
    letter-spacing: 0.02em;
    color: var(--accent);
  }
  .unread-dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--accent);
    flex: 0 0 auto;
  }
  .file {
    font-size: 11px;
    color: var(--text-tertiary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 40%;
  }

  .md {
    font-size: 13.5px;
    line-height: 1.68;
    color: var(--text);
    max-width: 66ch;
  }
  .md :global(h1) {
    margin: 0 0 8px;
    font-size: 15px;
    font-weight: 600;
    letter-spacing: -0.015em;
    line-height: 1.3;
  }
  .md :global(h2) {
    margin: 18px 0 8px;
    font-size: 13px;
    font-weight: 600;
    letter-spacing: -0.01em;
    color: var(--text);
  }
  .md :global(h3) {
    margin: 16px 0 6px;
    font-size: 13.5px;
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
    padding-left: 1.25em;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .md :global(a) {
    color: var(--accent);
    text-decoration: underline;
    text-decoration-thickness: 1px;
    text-underline-offset: 2px;
  }
  .md :global(a:hover) {
    color: var(--accent-hover);
  }
  .md :global(strong) {
    color: var(--text);
  }
  .md :global(blockquote) {
    margin: 0 0 12px;
    padding: 2px 0 2px 14px;
    border-left: 3px solid var(--line);
    color: var(--text-secondary);
  }
  .md :global(hr) {
    border: 0;
    border-top: 1px solid var(--line);
    margin: 18px 0;
  }
  .md :global(code) {
    font-family: var(--term);
    font-size: 0.88em;
    background: var(--bg-elevated);
    border-radius: 5px;
    padding: 1px 5px;
  }
  .md :global(pre) {
    background: var(--bg-elevated);
    border-radius: 12px;
    padding: 12px 14px;
    overflow-x: auto;
    margin: 0 0 12px;
  }
  .md :global(pre code) {
    background: none;
    padding: 0;
    font-size: 12.5px;
    line-height: 1.7;
    color: var(--text);
  }
  .md :global(img) {
    max-width: 100%;
    border-radius: 10px;
  }
  .md :global(table) {
    width: 100%;
    border-collapse: collapse;
    font-size: 13px;
    margin: 0 0 12px;
  }
  .md :global(th),
  .md :global(td) {
    text-align: left;
    padding: 6px 12px 6px 0;
    border-bottom: 1px solid var(--line);
  }

  .foot {
    text-align: center;
    font-size: 12px;
    line-height: 1.7;
    color: var(--text-tertiary);
  }

  @media (max-width: 720px) {
    .page-head {
      padding: 18px 20px 14px;
    }
    .scroll {
      padding: 4px 20px 44px;
    }
    .source {
      display: none;
    }
  }
</style>
