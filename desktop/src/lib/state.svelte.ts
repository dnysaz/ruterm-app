// Global app state (Svelte 5 runes) + the actions screens call.

import {
  api,
  errorText,
  type AppStatus,
  type Host,
  type Identity,
  type Note,
  type Vault,
  type VaultPayload,
} from './api';
import { clearAllSnapshots } from './scrollback';

export type Screen = 'loading' | 'welcome' | 'setup' | 'unlock' | 'vault';

export const app = $state({
  status: null as AppStatus | null,
  vault: null as Vault | null,
  revision: 0,
  screen: 'loading' as Screen,
  canAutoUnlock: false,
  busy: false,
  error: '',
  toast: '',
});

let toastTimer: ReturnType<typeof setTimeout> | null = null;

export function notify(message: string) {
  app.toast = message;
  if (toastTimer) clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (app.toast = ''), 4000);
}

export function fail(error: unknown) {
  app.error = errorText(error);
}

export async function bootstrap() {
  app.busy = true;
  try {
    app.status = await api.appStatus();
    if (!app.status.signed_in) {
      // Silent sign-in with the refresh token in the OS keychain.
      try {
        const account = await api.authRestore();
        app.status = await api.appStatus();
        if (account) notify(`Signed in as ${account.email}`);
      } catch {
        /* offline or stale token: keep going */
      }
    }

    if (app.status.unlocked) {
      const payload = await api.vaultGet();
      if (payload) {
        app.vault = payload.vault;
        app.revision = payload.revision;
        app.screen = 'vault';
        return;
      }
    }

    if (!app.status.has_vault) {
      app.screen = app.status.signed_in ? 'setup' : 'welcome';
      return;
    }

    // Never touch the keychain before the user asks: macOS would prompt on
    // every launch, and a fresh install has nothing cached anyway.
    app.canAutoUnlock = app.status.key_saved;
    app.screen = 'unlock';
  } catch (error) {
    fail(error);
    app.screen = 'welcome';
  } finally {
    app.busy = false;
  }
}

/** Opt in/out of caching the vault key in the OS keychain (F-31). */
export async function setRememberKey(enabled: boolean) {
  await api.vaultRememberKey(enabled);
  app.canAutoUnlock = enabled;
  if (app.status) app.status.key_saved = enabled;
  notify(enabled ? 'Vault key saved to the OS keychain' : 'Saved vault key removed');
}

/** Explicit one-click unlock with the key cached in the OS keychain. */
export async function unlockWithSavedKey() {
  app.busy = true;
  app.error = '';
  try {
    const payload = await api.vaultAutoUnlock();
    if (!payload) {
      // F-31: auto-unlock returns `None` both when no key is saved AND when a
      // saved key transiently fails to decrypt. Only clear the flag (and never
      // delete the keychain entry) when the key genuinely does not exist.
      let keyExists = true;
      try {
        keyExists = await api.vaultSavedKeyExists();
      } catch {
        /* assume the key is present: fail closed, keep it */
      }
      if (!keyExists) {
        // Genuinely gone (older build / cleared keychain) — safe to clear the
        // stale flag; the delete is a no-op on an already-empty entry.
        try {
          await api.vaultRememberKey(false);
        } catch {
          /* best effort */
        }
        app.canAutoUnlock = false;
        notify('No saved key on this device — enter your passphrase.');
      } else {
        // Key exists but could not unlock: leave it untouched.
        notify('Saved key could not open the vault — enter your passphrase.');
      }
      return;
    }
    app.vault = payload.vault;
    app.revision = payload.revision;
    app.screen = 'vault';
  } catch (error) {
    fail(error);
  } finally {
    app.busy = false;
  }
}

export async function signIn() {
  app.busy = true;
  app.error = '';
  try {
    await api.authSignIn();
    notify('Google sign-in complete');
    app.status = await api.appStatus();
    if (app.screen === 'welcome') app.screen = app.status.has_vault ? 'unlock' : 'setup';
  } catch (error) {
    fail(error);
  } finally {
    app.busy = false;
  }
}

export async function signOut() {
  app.busy = true;
  try {
    await api.authSignOut();
    clearAllSnapshots();
    app.status = await api.appStatus();
    notify('Signed out of Google');
  } catch (error) {
    fail(error);
  } finally {
    app.busy = false;
  }
}

export async function createVault(passphrase: string) {
  app.busy = true;
  app.error = '';
  try {
    const payload = await api.vaultCreate(passphrase);
    app.vault = payload.vault;
    app.revision = payload.revision;
    app.status = await api.appStatus();
    app.screen = 'vault';
    notify('Vault created');
  } catch (error) {
    fail(error);
  } finally {
    app.busy = false;
  }
}

/// Join the vault that already exists on the RuTerm bridge for this account.
export async function adoptRemote(passphrase: string) {
  app.busy = true;
  app.error = '';
  try {
    const payload = await api.vaultAdoptRemote(passphrase);
    app.vault = payload.vault;
    app.revision = payload.revision;
    app.status = await api.appStatus();
    app.screen = 'vault';
    notify('Vault retrieved from the sync bridge');
  } catch (error) {
    fail(error);
  } finally {
    app.busy = false;
  }
}

export async function unlock(passphrase: string) {
  app.busy = true;
  app.error = '';
  try {
    const payload = await api.vaultUnlock(passphrase);
    app.vault = payload.vault;
    app.revision = payload.revision;
    app.status = await api.appStatus();
    app.screen = 'vault';
  } catch (error) {
    fail(error);
  } finally {
    app.busy = false;
  }
}

export async function lock() {
  try {
    await api.vaultLock();
    clearAllSnapshots();
    app.vault = null;
    app.screen = 'unlock';
    app.status = await api.appStatus();
    notify('Vault locked');
  } catch (error) {
    fail(error);
  }
}

export async function saveVault(next: Vault) {
  const payload = await api.vaultSave({ ...next, updated_at: new Date().toISOString() });
  applyVaultPayload(payload);
  return payload.vault;
}

/// The backend already validated revision consistency; adopt its payload as-is.
export function applyVaultPayload(payload: VaultPayload) {
  app.vault = payload.vault;
  app.revision = payload.revision;
}

export async function syncNow() {
  app.busy = true;
  app.error = '';
  try {
    const payload = await api.vaultSync();
    app.vault = payload.vault;
    app.revision = payload.revision;
    app.status = await api.appStatus().catch(() => app.status);
    notify('Vault synced');
  } catch (error) {
    fail(error);
  } finally {
    app.busy = false;
  }
}

export async function saveHost(host: Host) {
  if (!app.vault) return;
  const hosts = app.vault.hosts.filter(h => h.id !== host.id);
  const updated = { ...host, updated_at: new Date().toISOString() };
  app.vault = await saveVault({ ...app.vault, hosts: [...hosts, updated] });
}

export async function deleteHost(id: string) {
  if (!app.vault) return;
  const hosts = app.vault.hosts.map(h =>
    h.id === id ? { ...h, deleted: true, updated_at: new Date().toISOString() } : h,
  );
  app.vault = await saveVault({ ...app.vault, hosts });
  notify('Host deleted');
}

export async function addIdentity(identity: Identity) {
  if (!app.vault) return;
  app.vault = await saveVault({
    ...app.vault,
    identities: [...app.vault.identities.filter(i => i.id !== identity.id), identity],
  });
}

export async function saveNote(note: Note) {
  if (!app.vault) return;
  const updated = { ...note, updated_at: new Date().toISOString() };
  app.vault = await saveVault({
    ...app.vault,
    notes: [...(app.vault.notes ?? []).filter(n => n.id !== note.id), updated],
  });
}

export async function deleteNote(id: string) {
  if (!app.vault) return;
  const notes = (app.vault.notes ?? []).map(n =>
    n.id === id ? { ...n, deleted: true, updated_at: new Date().toISOString() } : n,
  );
  app.vault = await saveVault({ ...app.vault, notes });
  notify('Note deleted');
}

/// Push the freshly saved vault to D1 for signed-in accounts, without toasts.
/// Failures are ignored: the encrypted local copy is already on disk.
let syncQueue: Promise<void> = Promise.resolve();

export function syncQuiet(): Promise<void> {
  if (!app.status?.signed_in) return Promise.resolve();
  syncQueue = syncQueue.then(async () => {
    try {
      const payload = await api.vaultSync();
      applyVaultPayload(payload);
      app.status = await api.appStatus().catch(() => app.status);
    } catch {
      /* offline — local vault remains authoritative until the next sync */
    }
  });
  return syncQueue;
}

export function hosts(): Host[] {
  return (app.vault?.hosts ?? []).filter(h => !h.deleted);
}

export function notes(): Note[] {
  return (app.vault?.notes ?? []).filter(n => !n.deleted);
}

export function identities(): Identity[] {
  return (app.vault?.identities ?? []).filter(i => !i.deleted);
}
