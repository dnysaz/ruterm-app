<script lang="ts">
  import { onMount } from 'svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';

  // macOS hides its native traffic lights in fullscreen. Show native-looking
  // controls in the webview so they stay visible in the top-left corner there.
  let visible = $state(false);
  let appWindow: ReturnType<typeof getCurrentWindow> | null = null;

  async function refresh() {
    if (!appWindow) return;
    try {
      visible = await appWindow.isFullscreen();
    } catch {
      visible = false;
    }
  }

  onMount(() => {
    try {
      appWindow = getCurrentWindow();
    } catch {
      return;
    }
    void refresh();
    window.addEventListener('resize', refresh);
    // Native fullscreen also changes the webview size, but poll as a fallback
    // for changes AppKit reports without a resize event.
    const timer = setInterval(() => void refresh(), 1500);
    return () => {
      window.removeEventListener('resize', refresh);
      clearInterval(timer);
    };
  });

  async function act(kind: 'close' | 'minimize' | 'exit') {
    if (!appWindow) return;
    try {
      if (kind === 'close') await appWindow.close();
      else if (kind === 'minimize') await appWindow.minimize();
      else await appWindow.setFullscreen(false);
    } catch (error) {
      console.warn('RuTerm could not control the window', error);
    }
    void refresh();
  }
</script>

{#if visible}
  <div class="traffic-lights" role="group" aria-label="Window controls">
    <button
      class="light close"
      type="button"
      title="Close window"
      aria-label="Close window"
      onclick={() => void act('close')}
    ><span aria-hidden="true">×</span></button>
    <button
      class="light minimize"
      type="button"
      title="Minimize window"
      aria-label="Minimize window"
      onclick={() => void act('minimize')}
    ><span aria-hidden="true">−</span></button>
    <button
      class="light exit"
      type="button"
      title="Exit full screen"
      aria-label="Exit full screen"
      onclick={() => void act('exit')}
    ><span aria-hidden="true">↙</span></button>
  </div>
{/if}

<style>
  .traffic-lights {
    position: fixed;
    top: 13px;
    left: 14px;
    z-index: 9999;
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .light {
    display: grid;
    place-items: center;
    width: 13px;
    height: 13px;
    padding: 0;
    border: 0.5px solid rgba(0, 0, 0, 0.2);
    border-radius: 50%;
    cursor: pointer;
    color: rgba(0, 0, 0, 0.6);
    font: 10px/1 system-ui, sans-serif;
  }
  .light span {
    opacity: 0;
    transition: opacity 0.12s ease;
  }
  .traffic-lights:hover .light span,
  .light:focus-visible span {
    opacity: 1;
  }
  .close { background: #ff5f57; }
  .minimize { background: #febc2e; }
  .exit { background: #28c840; }
</style>
