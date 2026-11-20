<script module lang="ts">
  type MonacoApi = typeof import('monaco-editor');

  let monacoPromise: Promise<MonacoApi> | null = null;

  /** Monaco is heavy and browser-only, so it is imported lazily, once, on mount. */
  async function loadMonaco(): Promise<MonacoApi> {
    if (!monacoPromise) {
      monacoPromise = (async () => {
        const [monaco, editorWorker, jsonWorker, cssWorker, htmlWorker, tsWorker] =
          await Promise.all([
            import('monaco-editor'),
            import('monaco-editor/editor/editor.worker?worker'),
            import('monaco-editor/language/json/json.worker?worker'),
            import('monaco-editor/language/css/css.worker?worker'),
            import('monaco-editor/language/html/html.worker?worker'),
            import('monaco-editor/language/typescript/ts.worker?worker'),
          ]);

        (globalThis as unknown as { MonacoEnvironment: unknown }).MonacoEnvironment = {
          getWorker(_workerId: string, label: string) {
            switch (label) {
              case 'json':
                return new jsonWorker.default();
              case 'css':
              case 'scss':
              case 'less':
                return new cssWorker.default();
              case 'html':
              case 'handlebars':
              case 'razor':
                return new htmlWorker.default();
              case 'typescript':
              case 'javascript':
                return new tsWorker.default();
              default:
                return new editorWorker.default();
            }
          },
        };

        defineThemes(monaco);
        return monaco;
      })();
    }
    return monacoPromise;
  }

  /** Both themes mirror the app's palette (app.css) so the editor never clashes. */
  function defineThemes(monaco: MonacoApi) {
    monaco.editor.defineTheme('rterm-dark', {
      base: 'vs-dark',
      inherit: true,
      rules: [
        { token: 'comment', foreground: '6a9955', fontStyle: 'italic' },
        { token: 'keyword', foreground: '569cd6' },
        { token: 'string', foreground: 'ce9178' },
        { token: 'number', foreground: 'b5cea8' },
        { token: 'regexp', foreground: 'd16969' },
        { token: 'operator', foreground: 'd4d4d4' },
        { token: 'delimiter', foreground: 'd4d4d4' },
        { token: 'type', foreground: '4ec9b0' },
        { token: 'type.identifier', foreground: '4ec9b0' },
        { token: 'namespace', foreground: '4ec9b0' },
        { token: 'function', foreground: 'dcdcaa' },
        { token: 'variable', foreground: '9cdcfe' },
        { token: 'variable.predefined', foreground: '4fc1ff' },
        { token: 'constant', foreground: '4fc1ff' },
        { token: 'tag', foreground: '569cd6' },
        { token: 'attribute.name', foreground: '9cdcfe' },
        { token: 'attribute.value', foreground: 'ce9178' },
        { token: 'metatag', foreground: '569cd6' },
        { token: 'key', foreground: '9cdcfe' },
        { token: 'key.json', foreground: '9cdcfe' },
        { token: 'invalid', foreground: 'ff453a' },
      ],
      colors: {
        'editor.background': '#141414',
        'editor.foreground': '#e6e6e6',
        'editorCursor.foreground': '#0a84ff',
        'editor.selectionBackground': '#264f78',
        'editor.inactiveSelectionBackground': '#3a3d4166',
        'editor.lineHighlightBackground': '#1b1b1b',
        'editor.lineHighlightBorder': '#00000000',
        'editorLineNumber.foreground': '#5c5c60',
        'editorLineNumber.activeForeground': '#c8c8cc',
        'editorIndentGuide.background1': '#262628',
        'editorIndentGuide.activeBackground1': '#3e3e42',
        'editorWhitespace.foreground': '#3b3b3d',
        'editorGutter.background': '#141414',
        'editorWidget.background': '#1e1e1e',
        'editorWidget.border': '#0a84ff4d',
        'editorSuggestWidget.background': '#1e1e1e',
        'editorSuggestWidget.border': '#0a84ff4d',
        'editorSuggestWidget.selectedBackground': '#264f78',
        'editorHoverWidget.background': '#1e1e1e',
        'editorHoverWidget.border': '#0a84ff4d',
        'editorBracketMatch.background': '#0a84ff26',
        'editorBracketMatch.border': '#0a84ff66',
        'editorOverviewRuler.border': '#00000000',
        'editorError.foreground': '#ff453a',
        'editorWarning.foreground': '#ff9f0a',
        'minimap.background': '#141414',
        'scrollbarSlider.background': '#ffffff1a',
        'scrollbarSlider.hoverBackground': '#ffffff2e',
        'scrollbarSlider.activeBackground': '#ffffff40',
      },
    });

    monaco.editor.defineTheme('rterm-light', {
      base: 'vs',
      inherit: true,
      rules: [
        { token: 'comment', foreground: '008000', fontStyle: 'italic' },
        { token: 'keyword', foreground: '0000ff' },
        { token: 'string', foreground: 'a31515' },
        { token: 'number', foreground: '098658' },
        { token: 'regexp', foreground: '811f3f' },
        { token: 'operator', foreground: '1c1c1e' },
        { token: 'delimiter', foreground: '1c1c1e' },
        { token: 'type', foreground: '267f99' },
        { token: 'type.identifier', foreground: '267f99' },
        { token: 'namespace', foreground: '267f99' },
        { token: 'function', foreground: '795e26' },
        { token: 'variable', foreground: '001080' },
        { token: 'variable.predefined', foreground: '0070c1' },
        { token: 'constant', foreground: '0070c1' },
        { token: 'tag', foreground: '800000' },
        { token: 'attribute.name', foreground: 'e50000' },
        { token: 'attribute.value', foreground: '0451a5' },
        { token: 'metatag', foreground: '800000' },
        { token: 'key', foreground: '0451a5' },
        { token: 'key.json', foreground: '0451a5' },
        { token: 'invalid', foreground: 'cd3131' },
      ],
      colors: {
        'editor.background': '#ffffff',
        'editor.foreground': '#1c1c1e',
        'editorCursor.foreground': '#0a84ff',
        'editor.selectionBackground': '#add6ff',
        'editor.inactiveSelectionBackground': '#e5ebf1',
        'editor.lineHighlightBackground': '#f6f6f8',
        'editor.lineHighlightBorder': '#00000000',
        'editorLineNumber.foreground': '#9a9aa0',
        'editorLineNumber.activeForeground': '#3c3c43',
        'editorIndentGuide.background1': '#e4e4e8',
        'editorIndentGuide.activeBackground1': '#c7c7cc',
        'editorWhitespace.foreground': '#d1d1d6',
        'editorGutter.background': '#ffffff',
        'editorWidget.background': '#f5f5f7',
        'editorWidget.border': '#0a84ff59',
        'editorSuggestWidget.background': '#f5f5f7',
        'editorSuggestWidget.border': '#0a84ff59',
        'editorSuggestWidget.selectedBackground': '#add6ff',
        'editorHoverWidget.background': '#f5f5f7',
        'editorHoverWidget.border': '#0a84ff59',
        'editorBracketMatch.background': '#0a84ff1f',
        'editorBracketMatch.border': '#0a84ff66',
        'editorOverviewRuler.border': '#00000000',
        'editorError.foreground': '#cd3131',
        'editorWarning.foreground': '#bf8803',
        'minimap.background': '#ffffff',
        'scrollbarSlider.background': '#0000001a',
        'scrollbarSlider.hoverBackground': '#0000002e',
        'scrollbarSlider.activeBackground': '#00000040',
      },
    });
  }
</script>

<script lang="ts">
  import { onMount } from 'svelte';
  import { ui } from '#lib/ui.svelte';
  import type * as Monaco from 'monaco-editor';

  let {
    value,
    language = 'plaintext',
    readonly = false,
    path = '',
    onchange,
    onsave,
    onescape,
  }: {
    /** Current document text. External changes replace the buffer. */
    value: string;
    /** Monaco language id, e.g. "typescript", "php", "rust". */
    language?: string;
    readonly?: boolean;
    /** Remote path, used only to give the model a stable URI. */
    path?: string;
    onchange?: (value: string) => void;
    /** Cmd/Ctrl+S inside the editor. */
    onsave?: () => void;
    /** Escape, handled before Monaco's own bindings. */
    onescape?: () => void;
  } = $props();

  type MonacoApi = typeof import('monaco-editor');

  let host = $state<HTMLDivElement | null>(null);
  let ready = $state(false);
  let monaco: MonacoApi | null = null;
  let editor: Monaco.editor.IStandaloneCodeEditor | null = null;
  let model: Monaco.editor.ITextModel | null = null;
  /** True while we push a new document into the model, so it is not "user typing". */
  let applyingExternal = false;

  function modelUri(api: MonacoApi): Monaco.Uri | undefined {
    if (!path) return undefined;
    try {
      return api.Uri.parse(`file://${encodeURI(path.startsWith('/') ? path : `/${path}`)}`);
    } catch {
      return undefined;
    }
  }

  function themeName(): string {
    return ui.theme === 'light' ? 'rterm-light' : 'rterm-dark';
  }

  onMount(() => {
    let disposed = false;

    void (async () => {
      const api = await loadMonaco();
      if (disposed || !host) return;
      monaco = api;

      const uri = modelUri(api);
      model = uri ? api.editor.createModel(value, language, uri) : api.editor.createModel(value, language);

      editor = api.editor.create(host, {
        model,
        theme: themeName(),
        readOnly: readonly,
        automaticLayout: true,
        fontSize: 13,
        lineHeight: 21,
        fontFamily:
          getComputedStyle(document.documentElement).getPropertyValue('--mono').trim() ||
          "'SF Mono', Menlo, monospace",
        fontLigatures: true,
        minimap: { enabled: true, renderCharacters: false, maxColumn: 90, showSlider: 'mouseover' },
        multiCursorModifier: 'alt',
        selectionHighlight: true,
        occurrencesHighlight: 'singleFile',
        renderLineHighlight: 'all',
        scrollBeyondLastLine: false,
        smoothScrolling: true,
        cursorBlinking: 'smooth',
        cursorSmoothCaretAnimation: 'on',
        bracketPairColorization: { enabled: true },
        guides: { bracketPairs: true, indentation: true },
        stickyScroll: { enabled: false },
        wordWrap: 'off',
        tabSize: 2,
        insertSpaces: true,
        detectIndentation: true,
        trimAutoWhitespace: true,
        formatOnPaste: true,
        renderWhitespace: 'selection',
        padding: { top: 12, bottom: 24 },
        scrollbar: { verticalScrollbarSize: 10, horizontalScrollbarSize: 10, useShadows: false },
        overviewRulerBorder: false,
        fixedOverflowWidgets: true,
        contextmenu: true,
        readOnlyMessage: { value: 'Click Edit to change this file.' },
        ariaLabel: path ? `Contents of ${path}` : 'File contents',
      });

      editor.onDidChangeModelContent(() => {
        if (applyingExternal || !editor) return;
        onchange?.(editor.getValue());
      });

      if (onsave) editor.addCommand(api.KeyMod.CtrlCmd | api.KeyCode.KeyS, () => onsave?.());
      if (onescape) editor.addCommand(api.KeyCode.Escape, () => onescape?.());

      ready = true;
    })();

    return () => {
      disposed = true;
      editor?.dispose();
      model?.dispose();
      editor = null;
      model = null;
      ready = false;
    };
  });

  // External content changes (file loaded, save accepted, cancel) replace the buffer.
  $effect(() => {
    const next = value;
    if (!ready || !editor || !model) return;
    if (model.getValue() !== next) {
      applyingExternal = true;
      model.setValue(next);
      applyingExternal = false;
    }
  });

  $effect(() => {
    const lang = language;
    if (!ready || !monaco || !model) return;
    if (model.getLanguageId() !== lang) monaco.editor.setModelLanguage(model, lang);
  });

  $effect(() => {
    const readOnly = readonly;
    if (!ready || !editor) return;
    editor.updateOptions({ readOnly });
    // Hand the caret to the editor as soon as editing begins.
    if (!readOnly) editor.focus();
  });

  $effect(() => {
    const theme = themeName();
    if (!ready || !monaco) return;
    monaco.editor.setTheme(theme);
  });
</script>

<div class="code-host" bind:this={host}></div>
{#if !ready}
  <div class="booting" aria-hidden="true">
    <div class="spinner"></div>
  </div>
{/if}

<style>
  .code-host {
    flex: 1 1 auto;
    min-height: 0;
    min-width: 0;
    width: 100%;
    overflow: hidden;
    background: var(--panel);
    position: relative;
  }
  .booting {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    background: var(--panel);
  }
  .spinner {
    width: 22px;
    height: 22px;
    border-radius: 50%;
    border: 2px solid var(--line);
    border-top-color: var(--accent);
    animation: spin 0.9s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .spinner {
      animation: none;
    }
  }
</style>
