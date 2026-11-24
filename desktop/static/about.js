const REPO = 'https://github.com/dnysaz/ruterm-app';

function invoke(command, args) {
  // `withGlobalTauri` is off, so the public window.__TAURI__ bridge is not
  // installed. The low-level IPC object injected by the runtime is enough for
  // this static page's two read-only commands.
  const internals = window.__TAURI_INTERNALS__;
  if (typeof internals?.invoke === 'function') return internals.invoke(command, args);
  const api = window.__TAURI__;
  if (typeof api?.core?.invoke === 'function') return api.core.invoke(command, args);
  throw new Error('Tauri IPC unavailable');
}

async function init() {
  try {
    const version = await invoke('app_version');
    if (typeof version === 'string' && version) {
      document.getElementById('version').textContent = `Version ${version}`;
    }
  } catch {
    /* keep the fallback version rendered in the markup */
  }

  const github = document.getElementById('github');
  if (github) {
    github.addEventListener('click', async () => {
      try {
        await invoke('open_external_link', { url: REPO });
      } catch {
        window.open(REPO, '_blank', 'noopener,noreferrer');
      }
    });
  }
}

void init();
