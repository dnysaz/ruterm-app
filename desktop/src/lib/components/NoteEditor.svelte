<script lang="ts">
  import { onDestroy, onMount, untrack } from 'svelte';
  import type { Note } from '#lib/api';
  import { terminalTint } from '#lib/background.svelte';
  import {
    colorizeCode,
    detectTypedCode,
    exitCodeBlock,
    handleBeforeInput,
    handleEnter,
    handlePaste,
    renderEditor,
    serializeEditor,
    formatInline,
  } from '#lib/markdown-editor';
  import { ui } from '#lib/ui.svelte';

  let {
    note,
    onsave,
    onclose,
  }: { note: Note; onsave: (body: string) => void | Promise<void>; onclose?: () => void } = $props();

  let editor: HTMLDivElement | null = $state(null);
  let page: HTMLDivElement | null = $state(null);
  let copyTarget: HTMLElement | null = $state(null);
  let copyStyle = $state('');
  let copied = $state(false);
  let saved = untrack(() => note.body);
  let lastId = untrack(() => note.id);
  let timer: ReturnType<typeof setTimeout> | null = null;
  let colorTimer: ReturnType<typeof setTimeout> | null = null;

  /** Same frosted-glass treatment the terminal uses. */
  const tint = $derived(terminalTint());

  function current(): string {
    return editor ? serializeEditor(editor) : saved;
  }

  function flush() {
    if (timer) {
      clearTimeout(timer);
      timer = null;
    }
    const body = current();
    if (body === saved) return;
    saved = body;
    void onsave(body);
  }

  function schedule() {
    if (timer) clearTimeout(timer);
    timer = setTimeout(flush, 700);
    if (colorTimer) clearTimeout(colorTimer);
    colorTimer = setTimeout(() => {
      if (editor) void colorizeCode(editor);
    }, 350);
  }

  onMount(() => {
    if (editor) renderEditor(editor, saved);
  });

  // Re-colour existing blocks when the app theme flips; Monaco inlines token
  // colours, so old spans keep the previous theme until they are rebuilt.
  $effect(() => {
    const theme = ui.theme;
    if (editor) void colorizeCode(editor);
  });

  function pointCopy(pre: HTMLElement | null) {
    copyTarget = pre;
    if (!pre || !page) return;
    const block = pre.getBoundingClientRect();
    const host = page.getBoundingClientRect();
    copyStyle = `top:${Math.max(4, block.top - host.top + 6)}px; right:${Math.max(4, host.right - block.right + 6)}px;`;
  }

  /** Clicking outside a code block returns the caret to normal text. */
  function onNotePointerDown(event: PointerEvent) {
    if (!editor) return;
    const target = event.target as HTMLElement | null;
    if (target?.closest('pre, .code-copy')) return;
    if (exitCodeBlock(editor)) {
      event.preventDefault();
      editor.focus();
    }
  }

  function onPointerMove(event: PointerEvent) {
    const element = event.target as HTMLElement | null;
    if (element?.closest('.code-copy')) return;
    pointCopy((element?.closest('pre') as HTMLElement | null) ?? null);
  }

  async function copyCode() {
    if (!copyTarget) return;
    const code = copyTarget.querySelector('code');
    try {
      await navigator.clipboard.writeText(code?.textContent ?? '');
      copied = true;
      setTimeout(() => { copied = false; }, 1200);
    } catch {
      /* clipboard unavailable — leave the code text untouched */
    }
  }

  // Local AI agents can edit this note through the MCP bridge while its tab is
  // already open. Adopt their changes, but never clobber unsaved local typing.
  $effect(() => {
    const incoming = note.body;
    const id = note.id;
    if (!editor) return;
    if (id !== lastId) {
      lastId = id;
      saved = incoming;
      renderEditor(editor, incoming);
      return;
    }
    if (incoming === saved) return;
    if (current() !== saved) return;
    saved = incoming;
    renderEditor(editor, incoming);
  });

  $effect(() => {
    const onBeforeUnload = () => flush();
    window.addEventListener('beforeunload', onBeforeUnload);
    return () => window.removeEventListener('beforeunload', onBeforeUnload);
  });

  onDestroy(() => {
    if (timer) clearTimeout(timer);
    if (colorTimer) clearTimeout(colorTimer);
    flush();
  });
</script>

<svelte:window
  onkeydown={(event) => {
    if (event.key !== 'Escape') return;
    if ((event.target as HTMLElement | null)?.closest?.('.note-body')) return;
    (document.activeElement as HTMLElement | null)?.blur();
  }}
/>

<div
  class="note-page"
  role="presentation"
  bind:this={page}
  style:--term-alpha={`${tint.alpha}%`}
  style:--term-blur={`${tint.blur}px`}
  onpointerdown={onNotePointerDown}
  onpointermove={onPointerMove}
  onpointerleave={() => pointCopy(null)}
>
  <div
    class="note-body"
    bind:this={editor}
    contenteditable="true"
    spellcheck="false"
    role="textbox"
    tabindex="0"
    aria-label="Note"
    aria-multiline="true"
    data-placeholder="Write something…"
    onbeforeinput={(event) => {
      if (editor && handleBeforeInput(event, editor)) schedule();
    }}
    oninput={(event) => {
      if (!editor) return;
      if (!detectTypedCode(event, editor)) formatInline(editor);
      schedule();
    }}
    onkeydown={(event) => {
      if (!editor) return;
      if (event.key === 'Escape') {
        if (!exitCodeBlock(editor)) (document.activeElement as HTMLElement | null)?.blur();
        return;
      }
      if (handleEnter(event, editor)) schedule();
    }}
    onpaste={(event) => {
      if (editor && handlePaste(event, editor)) schedule();
    }}
    onblur={flush}
    onscroll={() => pointCopy(copyTarget)}
  ></div>
  {#if copyTarget}
    <button
      class="code-copy"
      class:copied
      style={copyStyle}
      type="button"
      aria-label="Copy code"
      onclick={() => void copyCode()}
    >{copied ? 'Copied' : 'Copy'}</button>
  {/if}
</div>

<style>
  .note-page {
    position: relative;
    height: 100%;
    overflow: hidden;
    background: color-mix(in srgb, var(--panel) var(--term-alpha, 100%), transparent);
    backdrop-filter: blur(var(--term-blur, 0px)) saturate(125%);
    -webkit-backdrop-filter: blur(var(--term-blur, 0px)) saturate(125%);
  }

  .note-body {
    height: 100%;
    padding: 24px 32px 48px;
    overflow-y: auto;
    outline: none;
    color: var(--text);
    font-family: var(--sans);
    font-size: 15px;
    line-height: 1.75;
    letter-spacing: -0.006em;
    caret-color: var(--accent);
    overflow-wrap: anywhere;
  }
  .note-body:empty::before {
    content: attr(data-placeholder);
    color: var(--text-tertiary);
    pointer-events: none;
  }

  .note-body :global(p) { margin: 0 0 12px; }
  .note-body :global(h1),
  .note-body :global(h2),
  .note-body :global(h3),
  .note-body :global(h4),
  .note-body :global(h5),
  .note-body :global(h6) {
    margin: 18px 0 8px;
    line-height: 1.35;
    font-weight: 650;
  }
  .note-body :global(h1) { font-size: 1.7em; }
  .note-body :global(h2) { font-size: 1.45em; }
  .note-body :global(h3) { font-size: 1.2em; }
  .note-body :global(ul),
  .note-body :global(ol) { margin: 0 0 12px; padding-left: 26px; }
  .note-body :global(li) { margin: 2px 0; }
  .note-body :global(li::marker) { color: var(--text-secondary); }
  .note-body :global(a) {
    color: var(--accent);
    text-decoration: underline;
    text-underline-offset: 2px;
  }
  .note-body :global(code) {
    font-family: var(--mono);
    font-size: 0.88em;
    padding: 1px 5px;
    border-radius: 4px;
    background: color-mix(in srgb, var(--bg-elevated) 80%, transparent);
    border: 1px solid var(--line);
  }
  .note-body :global(pre) {
    display: block;
    width: 100%;
    box-sizing: border-box;
    margin: 0 0 12px;
    margin-left: 0;
    padding: 12px 14px;
    overflow-x: auto;
    border: 1px solid var(--line);
    border-radius: 10px;
    background: color-mix(in srgb, var(--bg-elevated) 80%, transparent);
    font-family: var(--mono);
    font-size: 12.5px;
    line-height: 1.65;
    tab-size: 2;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .note-body :global(pre code) {
    padding: 0;
    border: 0;
    background: transparent;
    font: inherit;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .note-body :global(blockquote) {
    margin: 0 0 12px;
    padding-left: 12px;
    border-left: 3px solid var(--line);
    color: var(--text-secondary);
  }
  .note-body :global(hr) {
    margin: 18px 0;
    border: 0;
    border-top: 1px solid var(--line);
  }
  .note-body :global(table) { width: 100%; margin: 0 0 12px; border-collapse: collapse; }
  .note-body :global(th),
  .note-body :global(td) { padding: 6px 8px; border: 1px solid var(--line); text-align: left; }

  .code-copy {
    position: absolute;
    z-index: 3;
    padding: 3px 9px;
    border: 1px solid var(--line);
    border-radius: 6px;
    background: color-mix(in srgb, var(--panel) 92%, transparent);
    color: var(--text-secondary);
    font-family: var(--sans);
    font-size: 10.5px;
    font-weight: 600;
    cursor: pointer;
  }
  .code-copy:hover { border-color: var(--accent); color: var(--accent); }
  .code-copy.copied { color: var(--accent); }
</style>
