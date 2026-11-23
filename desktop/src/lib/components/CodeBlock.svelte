<script lang="ts">
  import { loadMonaco } from './CodeEditor.svelte';
  import { languageForLabel } from '#lib/languages';
  import { ui } from '#lib/ui.svelte';

  /** Read-only, Monaco-colourised code for note previews and AI chat replies. */
  let {
    code,
    label = 'code',
  }: {
    code: string;
    label?: string;
  } = $props();

  let html = $state('');

  function escapeHtml(value: string): string {
    return value.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
  }

  $effect(() => {
    const source = code ?? '';
    const lang = languageForLabel(label);
    const theme = ui.theme === 'light' ? 'rterm-light' : 'rterm-dark';
    let cancelled = false;

    void (async () => {
      try {
        const monaco = await loadMonaco();
        monaco.editor.setTheme(theme);
        const colored = await monaco.editor.colorize(source, lang, { tabSize: 2 });
        if (!cancelled) html = colored;
      } catch {
        // Monaco is optional eye candy: plain text is still correct.
        if (!cancelled) html = escapeHtml(source);
      }
    })();

    return () => {
      cancelled = true;
    };
  });
</script>

<pre class="code-block"><code>{@html html || escapeHtml(code ?? '')}</code></pre>

<style>
  .code-block {
    margin: 0;
    padding: 12px 14px;
    overflow-x: auto;
    background: transparent;
    color: var(--text);
    font-family: var(--mono);
    font-size: 12.5px;
    line-height: 1.65;
    tab-size: 2;
  }
  .code-block :global(code) {
    font: inherit;
    white-space: pre;
  }
</style>
