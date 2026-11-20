<script lang="ts">
  import Icon from './Icon.svelte';
  import ConfirmDialog from './ConfirmDialog.svelte';
  import { externalLink, closeExternalLink } from '#lib/externalLink.svelte';
  import { openExternalLink } from '#lib/external';

  async function confirmOpen() {
    const href = externalLink.href;
    externalLink.href = null;
    if (!href) return;
    await openExternalLink(href);
  }
</script>

{#if externalLink.href}
  <ConfirmDialog
    title="Open external link?"
    confirmLabel="Open link"
    oncancel={closeExternalLink}
    onconfirm={() => void confirmOpen()}
  >
    <p class="muted">RuTerm will open this address in your default browser:</p>
    <p class="url mono">{externalLink.href}</p>
    <p class="tiny muted hint"><Icon name="shield" size={13} /> Only open links from sources you trust.</p>
  </ConfirmDialog>
{/if}

<style>
  .url {
    margin: 10px 0;
    padding: 10px 12px;
    border-radius: 8px;
    background: var(--bg-elevated);
    border: 1px solid var(--line);
    font-size: 12px;
    line-height: 1.5;
    word-break: break-all;
    color: var(--text);
  }
  .hint {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    margin-top: 6px;
  }
</style>
