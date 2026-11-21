<script lang="ts">
  type Name =
    | 'settings'
    | 'sync'
    | 'lock'
    | 'search'
    | 'plus'
    | 'close'
    | 'key'
    | 'trash'
    | 'terminal'
    | 'chevron-right'
    | 'chevron-down'
    | 'ellipsis'
    | 'panel-left'
    | 'panel-right'
    | 'panel-top'
    | 'panel-bottom'
    | 'copy'
    | 'columns'
    | 'rows'
    | 'check'
    | 'alert'
    | 'edit'
    | 'info'
    | 'file-text'
    | 'shield'
    | 'mail'
    | 'code'
    | 'download'
    | 'home'
    | 'spark'
    | 'folder'
    | 'folder-open'
    | 'file'
    | 'upload'
    | 'arrow-up'
    | 'send'
    | 'stop'
    | 'refresh'
    | 'maximize'
    | 'minimize'
    | 'eye'
    | 'eye-off';

  let {
    name,
    size = 16,
    stroke = 1.7,
    fill = 'none',
  }: { name: Name; size?: number; stroke?: number; fill?: string } = $props();

  const paths: Record<string, string> = {
    sync: 'M3 12a9 9 0 0 1 15-6.7L21 8M21 12a9 9 0 0 1-15 6.7L3 16M21 3v5h-5M3 21v-5h5',
    lock: 'M6.5 10.5h11a2.5 2.5 0 0 1 2.5 2.5v5.5a2.5 2.5 0 0 1-2.5 2.5h-11A2.5 2.5 0 0 1 4 18.5V13a2.5 2.5 0 0 1 2.5-2.5zM8.5 10.5V7a3.5 3.5 0 1 1 7 0v3.5',
    plus: 'M12 5.5v13M5.5 12h13',
    close: 'M17.5 6.5 6.5 17.5M6.5 6.5l11 11',
    key: 'M15 8.5a3.5 3.5 0 1 1-4.9 3.2L4.5 17.2v3.3h3.3v-2h2v-2h2l1.3-1.3',
    trash:
      'M4 7h16M9.5 7V5.5a1 1 0 0 1 1-1h3a1 1 0 0 1 1 1V7M18.5 7l-.8 12.2a2 2 0 0 1-2 1.8H8.3a2 2 0 0 1-2-1.8L5.5 7M10 11v6M14 11v6',
    terminal: 'M7.5 10 10 12.5 7.5 15M13 15h3.5',
    'chevron-right': 'm9.5 6.5 5.5 5.5-5.5 5.5',
    'chevron-down': 'm6.5 9.5 5.5 5.5 5.5-5.5',
    'panel-left': 'M4.6 4.5h14.8A1.6 1.6 0 0 1 21 6.1v11.8a1.6 1.6 0 0 1-1.6 1.6H4.6A1.6 1.6 0 0 1 3 17.9V6.1a1.6 1.6 0 0 1 1.6-1.6zM9 4.5v15',
    'panel-right': 'M4.6 4.5h14.8A1.6 1.6 0 0 1 21 6.1v11.8a1.6 1.6 0 0 1-1.6 1.6H4.6A1.6 1.6 0 0 1 3 17.9V6.1a1.6 1.6 0 0 1 1.6-1.6zM15 4.5v15',
    'panel-top': 'M4.6 4.5h14.8A1.6 1.6 0 0 1 21 6.1v11.8a1.6 1.6 0 0 1-1.6 1.6H4.6A1.6 1.6 0 0 1 3 17.9V6.1a1.6 1.6 0 0 1 1.6-1.6zM3 9.4h18',
    'panel-bottom': 'M4.6 4.5h14.8A1.6 1.6 0 0 1 21 6.1v11.8a1.6 1.6 0 0 1-1.6 1.6H4.6A1.6 1.6 0 0 1 3 17.9V6.1a1.6 1.6 0 0 1 1.6-1.6zM3 14.6h18',
    copy: 'M11.5 9.5h6a2 2 0 0 1 2 2v6a2 2 0 0 1-2 2h-6a2 2 0 0 1-2-2v-6a2 2 0 0 1 2-2zM14.8 9.5V6.8a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v7a2 2 0 0 0 2 2h2.9',
    columns: 'M4.5 5h15a1 1 0 0 1 1 1v12a1 1 0 0 1-1 1h-15a1 1 0 0 1-1-1V6a1 1 0 0 1 1-1zM12.25 5v14',
    rows: 'M4.5 5h15a1 1 0 0 1 1 1v12a1 1 0 0 1-1 1h-15a1 1 0 0 1-1-1V6a1 1 0 0 1 1-1zM3.5 12.25h17',
    check: 'M20 6.5 9.5 17 4 11.5',
    edit: 'M17 3a2.85 2.83 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5L17 3z',
    info: 'M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM12 11v5M12 7.5h.01',
    'file-text':
      'M13.5 3H7a1.5 1.5 0 0 0-1.5 1.5v15A1.5 1.5 0 0 0 7 21h10a1.5 1.5 0 0 0 1.5-1.5V7L13.5 3zM13.5 3v4h4.5M9.5 12.5h5M9.5 16h5',
    shield: 'M12 3l7 2.8V12c0 4.4-2.8 7.6-7 9-4.2-1.4-7-4.6-7-9V5.8L12 3z',
    mail: 'M4 6h16a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2zM4 8l8 5 8-5',
    code: 'm9 8-4 4 4 4M15 8l4 4-4 4',
    download: 'M12 3.5v11m0 0 4-4m-4 4-4-4M4.5 17v1.5A2.5 2.5 0 0 0 7 21h10a2.5 2.5 0 0 0 2.5-2.5V17',
    home: 'M4 10.5 12 4l8 6.5V19a1.5 1.5 0 0 1-1.5 1.5h-13A1.5 1.5 0 0 1 4 19v-8.5zM9.5 20.5V14h5v6.5',
    spark: 'M12 2.5c.55 5 4.45 8.9 9.5 9.5-5.05.6-8.95 4.5-9.5 9.5-.55-5-4.45-8.9-9.5-9.5 5.05-.6 8.95-4.5 9.5-9.5z',
    alert: 'M12 8.5v4.5M12 17h.01M10.4 4.2 2.6 18a1.9 1.9 0 0 0 1.7 2.8h15.4a1.9 1.9 0 0 0 1.7-2.8L13.6 4.2a1.9 1.9 0 0 0-3.2 0z',
    folder: 'M4 8.5a2 2 0 0 1 2-2h3.7l1.7 2H18a2 2 0 0 1 2 2v7a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V8.5z',
    'folder-open': 'M3.5 8.5a2 2 0 0 1 2-2h3.7l1.7 2h6a2 2 0 0 1 2 2v1M3.5 10.5h15l-1.2 6a2 2 0 0 1-2 1.5H6.8a2 2 0 0 1-2-1.7l-1.3-5.8z',
    file: 'M13.5 3H7a1.5 1.5 0 0 0-1.5 1.5v15A1.5 1.5 0 0 0 7 21h10a1.5 1.5 0 0 0 1.5-1.5V7L13.5 3zM13.5 3v4h4.5',
    upload: 'M12 15.5v-12m0 0L8 7.5m4-4 4 4M4.5 17v1.5A2.5 2.5 0 0 0 7 20.5h10a2.5 2.5 0 0 0 2.5-2.5V17',
    'arrow-up': 'M12 19V5m0 0L6.5 10.5M12 5l5.5 5.5',
    send: 'M21 3 3 10.5l7.5 2.5L13 20.5 21 3zM10.5 13 21 3',
    stop: 'M7 7h10v10H7z',
    refresh: 'M20 12a8 8 0 1 1-2.34-5.66L20 8M20 3v5h-5',
    maximize: 'M8 3H3v5M16 3h5v5M21 16v5h-5M3 16v5h5',
    minimize: 'M3 8h5V3M21 8h-5V3M21 16h-5v5M3 16h5v5',
    eye: 'M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12zM9.25 12a2.75 2.75 0 1 0 5.5 0 2.75 2.75 0 0 0-5.5 0',
    'eye-off':
      'M9.9 4.9A10 10 0 0 1 12 4.5C18 4.5 21.5 12 21.5 12a17.6 17.6 0 0 1-3.04 4.18M14.42 14.42a3.5 3.5 0 0 1-4.84-4.84M6.61 6.61A17.3 17.3 0 0 0 2.5 12S6 19.5 12 19.5a10.2 10.2 0 0 0 3.8-.75M3 3l18 18',
  };
</script>

<svg
  width={size}
  height={size}
  viewBox="0 0 24 24"
  {fill}
  stroke="currentColor"
  stroke-width={stroke}
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
>
  {#if name === 'settings'}
    <path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z" />
    <circle cx="12" cy="12" r="3" />
  {:else if name === 'search'}
    <circle cx="11" cy="11" r="6.3" />
    <path d="m20 20-3.7-3.7" />
  {:else if name === 'ellipsis'}
    <circle cx="5.5" cy="12" r="1.4" fill="currentColor" stroke="none" />
    <circle cx="12" cy="12" r="1.4" fill="currentColor" stroke="none" />
    <circle cx="18.5" cy="12" r="1.4" fill="currentColor" stroke="none" />
  {:else if name === 'terminal'}
    <rect x="3" y="4.5" width="18" height="15" rx="3" />
    <path d={paths.terminal} />
  {:else if name === 'lock'}
    <path d={paths.lock} />
    <circle cx="12" cy="15.7" r="1.05" fill="currentColor" stroke="none" />
  {:else}
    <path d={paths[name]} />
  {/if}
</svg>
