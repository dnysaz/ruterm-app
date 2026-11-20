const REPO = 'https://github.com/dnysaz/ruterm-app';

function tauri() {
  return typeof window.__TAURI__ !== 'undefined' ? window.__TAURI__ : null;
}

async function invoke(command, args) {
  const api = tauri();
  if (!api?.core?.invoke) throw new Error('Tauri IPC unavailable');
  return api.core.invoke(command, args);
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
