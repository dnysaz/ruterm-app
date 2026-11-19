<script lang="ts">
  import type { Host } from '$lib/api';

  import OsIcon from './OsIcon.svelte';

  let { host, size = 30 }: { host: Host | null; size?: number } = $props();

  function initials(h: Host | null): string {
    if (!h) return '?';
    const text = (h.label || h.address || '?').trim();
    const letters = text
      .split(/[\s\-_./]+/)
      .filter(Boolean)
      .slice(0, 2)
      .map(part => part[0]?.toUpperCase() ?? '')
      .join('');
    return letters || (text[0]?.toUpperCase() ?? '?');
  }

  const knownOs = $derived.by(() => {
    const os = (host?.os ?? '').toLowerCase();
    return os && os !== 'unknown' ? os : null;
  });
</script>

{#if knownOs}
  <OsIcon os={knownOs} {size} />
{:else}
  <span class="avatar mono" style="width:{size}px;height:{size}px;border-radius:{Math.round(size * 0.28)}px;font-size:{Math.round(size * 0.42)}px">{initials(host)}</span>
{/if}

<style>
  .avatar {
    flex: 0 0 auto;
    display: grid;
    place-items: center;
    font-weight: 650;
    color: var(--text);
    background: var(--panel-hover);
    border: 1px solid var(--line);
  }
</style>
