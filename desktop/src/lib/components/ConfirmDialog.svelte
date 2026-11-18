<script lang="ts">
  import { onMount, tick } from 'svelte';
  import Icon from './Icon.svelte';
  import type { Snippet } from 'svelte';

  interface Props {
    /** Short verb-led heading, e.g. "Delete this host?" */
    title: string;
    /** Body copy / detail; keep to one or two sentences. */
    children?: Snippet;
    confirmLabel?: string;
    cancelLabel?: string;
    onconfirm: () => void;
    oncancel: () => void;
  }

  let {
    title,
    children,
    confirmLabel = 'Delete',
    cancelLabel = 'Cancel',
    onconfirm,
    oncancel,
  }: Props = $props();

  let cancelBtn: HTMLButtonElement;

  onMount(() => {
    // Safe default: focus lands on Cancel, never on the destructive action.
    void tick().then(() => cancelBtn?.focus());
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        oncancel();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  });
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<div
  class="scrim"
  role="presentation"
  onmousedown={event => event.target === event.currentTarget && oncancel()}
>
  <div
    class="confirm-dialog"
    role="alertdialog"
    aria-modal="true"
    aria-labelledby="confirm-title"
  >
    <div class="dialog-icon" aria-hidden="true">
      <Icon name="alert" size={21} />
    </div>
    <h2 id="confirm-title">{title}</h2>
    {#if children}
      <div class="dialog-body">{@render children()}</div>
    {/if}
    <div class="dialog-actions">
      <button class="btn ghost" type="button" bind:this={cancelBtn} onclick={oncancel}>
        {cancelLabel}
      </button>
      <button class="btn danger-solid" type="button" onclick={onconfirm}>
        <Icon name="trash" size={15} />
        {confirmLabel}
      </button>
    </div>
  </div>
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 900;
    display: grid;
    place-items: center;
    padding: 28px;
    background: var(--scrim);
    backdrop-filter: blur(7px) saturate(0.85);
    -webkit-backdrop-filter: blur(7px) saturate(0.85);
    animation: scrim-in 0.16s ease-out;
  }

  .confirm-dialog {
    width: min(430px, 100%);
    border: 1px solid var(--line-strong);
    border-radius: 16px;
    background: var(--glass-strong);
    box-shadow:
      0 32px 72px rgba(0, 0, 0, 0.5),
      inset 0 1px 0 rgba(255, 255, 255, 0.06);
    padding: 26px 26px 20px;
    text-align: center;
    animation: dialog-in 0.22s cubic-bezier(0.2, 1.15, 0.4, 1);
  }

  @keyframes scrim-in {
    from {
      opacity: 0;
    }
  }
  @keyframes dialog-in {
    from {
      opacity: 0;
      transform: translateY(10px) scale(0.96);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .scrim,
    .confirm-dialog {
      animation: none;
    }
  }

  .dialog-icon {
    display: grid;
    place-items: center;
    width: 46px;
    height: 46px;
    margin: 0 auto 13px;
    border-radius: 50%;
    border: 1px solid rgba(239, 111, 92, 0.35);
    background: rgba(239, 111, 92, 0.12);
    color: var(--bad);
  }

  h2 {
    margin: 0;
    font-size: 19px;
    font-weight: 600;
    letter-spacing: -0.01em;
  }

  .dialog-body {
    margin: 9px 2px 0;
    color: var(--muted);
    font-size: 14.5px;
    line-height: 1.6;
  }
  .dialog-body :global(strong) {
    color: var(--text);
    font-weight: 600;
  }

  .dialog-actions {
    display: flex;
    gap: 10px;
    margin-top: 20px;
  }
  .dialog-actions .btn {
    flex: 1;
    justify-content: center;
    height: 38px;
  }

  .danger-solid {
    color: #fff;
    border-color: transparent;
    background: var(--bad);
  }
  .danger-solid:hover {
    filter: brightness(1.08);
    background: var(--bad);
  }
  .danger-solid:active {
    filter: brightness(0.94);
  }
</style>
