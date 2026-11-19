<script lang="ts">
  import { onDestroy } from 'svelte';
  import { marked } from 'marked';
  import DOMPurify from 'dompurify';
  import type { Note } from '$lib/api';
  import { terminalTint } from '$lib/background.svelte';

  let {
    note,
    onsave,
    onclose,
  }: { note: Note; onsave: (body: string) => void | Promise<void>; onclose?: () => void } = $props();

  // Start in edit mode when the note is empty (a new note), otherwise read mode.
  let editing = $state(note.body.trim().length === 0);
  let text = $state(note.body);
  let saved = note.body;
  let lastId = note.id;
  let timer: ReturnType<typeof setTimeout> | null = null;

  function flush() {
    if (timer) {
      clearTimeout(timer);
      timer = null;
    }
    if (text === saved) return;
    saved = text;
    void onsave(text);
  }

  function schedule() {
    if (timer) clearTimeout(timer);
    timer = setTimeout(flush, 700);
  }

  function doBlur() {
    flush();
    if (text.trim().length > 0) editing = false;
  }

  function renderMarkdown(markdown: string): string {
    const html = marked.parse(markdown || '', { gfm: true, breaks: true }) as string;
    if (typeof DOMPurify?.sanitize === 'function') return DOMPurify.sanitize(html);
    return html;
  }

  /** Same frosted-glass treatment the terminal uses: backdrop (if any) bleeds
   *  through the note surface, driven by the "Terminal" Dim slider. */
  const tint = $derived(terminalTint());

  $effect(() => {
    // onDestroy won't fire during a full page reload — flush pending edits.
    const onBeforeUnload = () => flush();
    window.addEventListener('beforeunload', onBeforeUnload);
    return () => window.removeEventListener('beforeunload', onBeforeUnload);
  });

  onDestroy(() => {
    flush();
  });
</script>

<svelte:window
  onkeydown={(event) => {
    if (event.key === 'Escape' && editing) {
      event.preventDefault();
      event.stopPropagation();
      (document.activeElement as HTMLElement | null)?.blur();
    }
  }}
/>

<div class="note-page" style:--term-alpha={`${tint.alpha}%`} style:--term-blur={`${tint.blur}px`}>
  {#if editing}
    <textarea
      class="note-body"
      spellcheck="false"
      placeholder="Write something…"
      aria-label="Note"
      bind:value={text}
      oninput={schedule}
      onblur={doBlur}
    ></textarea>
  {:else}
    <div
      class="note-render"
      role="textbox"
      tabindex="0"
      aria-label="Note — press Enter to edit"
      onclick={() => (editing = true)}
      onkeydown={(event) => (event.key === 'Enter' || event.key === ' ') && (editing = true)}
    >
      <div class="md">{@html renderMarkdown(text)}</div>
    </div>
  {/if}
</div>

<style>
  .note-page {
    height: 100%;
    display: flex;
    overflow: hidden;
    background: color-mix(in srgb, var(--panel) var(--term-alpha, 100%), transparent);
    backdrop-filter: blur(var(--term-blur, 0px)) saturate(125%);
    -webkit-backdrop-filter: blur(var(--term-blur, 0px)) saturate(125%);
  }

  .note-body {
    flex: 1;
    width: 100%;
    height: 100%;
    padding: 24px 32px 48px;
    border: 0;
    outline: none;
    resize: none;
    overflow-y: auto;
    background: transparent;
    color: var(--text);
    font-family: var(--sans);
    font-size: 15px;
    line-height: 1.7;
    letter-spacing: -0.006em;
    caret-color: var(--accent);
    -webkit-appearance: none;
  }
  .note-body::placeholder {
    color: var(--text-tertiary);
  }

  .note-render {
    flex: 1;
    width: 100%;
    height: 100%;
    padding: 24px 32px 48px;
    overflow-y: auto;
    background: transparent;
    color: var(--text);
    cursor: text;
    outline: none;
  }
  .note-render:focus-visible {
    outline: none;
  }

  .md {
    max-width: none;
    font-family: var(--sans);
    font-size: 15px;
    line-height: 1.75;
    color: var(--text);
    overflow-wrap: anywhere;
  }

  .md :global(p) {
    margin: 0 0 12px;
  }
  .md :global(ul),
  .md :global(ol) {
    margin: 0 0 12px;
    padding-left: 22px;
  }
  .md :global(li) {
    margin: 2px 0;
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
  .md :global(code) {
    font-family: var(--mono);
    font-size: 0.88em;
    padding: 1px 5px;
    border-radius: 4px;
    background: color-mix(in srgb, var(--bg-elevated) 80%, transparent);
    border: 1px solid var(--line);
  }
  .md :global(pre) {
    margin: 0 0 12px;
    padding: 12px 14px;
    border-radius: 10px;
    background: color-mix(in srgb, var(--bg-elevated) 80%, transparent);
    border: 1px solid var(--line);
    overflow-x: auto;
    font-family: var(--mono);
    font-size: 12px;
    line-height: 1.65;
  }
  .md :global(pre code) {
    padding: 0;
    border: 0;
    background: transparent;
    font-size: inherit;
  }
  .md :global(blockquote) {
    margin: 0 0 12px;
    padding-left: 12px;
    border-left: 3px solid var(--line);
    color: var(--text-secondary);
  }
  .md :global(hr) {
    margin: 18px 0;
    border: 0;
    border-top: 1px solid var(--line);
  }
</style>
