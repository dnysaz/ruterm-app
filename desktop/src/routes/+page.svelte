<script lang="ts">
  import { onMount } from 'svelte';
  import { app, bootstrap } from '$lib/state.svelte';
  import { installShortcuts } from '$lib/keys';
  import { restoreTheme, restoreZoom } from '$lib/ui.svelte';
  import Welcome from '$lib/components/Welcome.svelte';
  import Unlock from '$lib/components/Unlock.svelte';
  import VaultShell from '$lib/components/VaultShell.svelte';

  onMount(() => {
    restoreTheme();
    restoreZoom();
    const remove = installShortcuts();
    void bootstrap();
    return remove;
  });
</script>

<!-- The window uses an overlay title bar, so this strip is the drag handle. -->
<div class="titlebar" data-tauri-drag-region></div>

<div class="root">
  {#if app.screen === 'loading'}
    <div class="splash">
      <div class="mark">r</div>
      <div class="name">RuTerm<span class="caret" aria-hidden="true">_</span></div>
      <div class="muted small">Opening your vault…</div>
    </div>
  {:else if app.screen === 'welcome'}
    <Welcome />
  {:else if app.screen === 'setup' || app.screen === 'unlock'}
    <Unlock />
  {:else if app.screen === 'vault'}
    <VaultShell />
  {/if}
</div>

{#if app.toast}
  <div class="toast">{app.toast}</div>
{/if}
{#if app.error}
  <div
    class="toast error"
    role="button"
    tabindex="0"
    onclick={() => (app.error = '')}
    onkeydown={event => event.key === 'Enter' && (app.error = '')}
  >
    {app.error}
  </div>
{/if}

<style>
  .titlebar {
    position: fixed;
    top: 0;
    left: 0;
    right: 0;
    height: 30px;
    z-index: 70;
  }

  .root {
    height: 100%;
  }

  .splash {
    height: 100%;
    padding-top: 30px;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 6px;
  }
  .mark {
    width: 46px;
    height: 46px;
    display: grid;
    place-items: center;
    border-radius: 13px;
    border: 1px solid var(--accent-line);
    background: var(--accent-dim);
    color: var(--accent);
    font-family: var(--mono);
    font-size: 23.5px;
    font-weight: 600;
    margin-bottom: 10px;
  }
  .name {
    font-family: var(--term);
    font-size: 18.5px;
    font-weight: 700;
    letter-spacing: -0.01em;
  }
  .caret {
    color: var(--accent);
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
  .small {
    font-size: 14px;
  }
</style>
