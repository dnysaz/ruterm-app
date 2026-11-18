// Typed bridge to the Tauri commands (src-tauri/src/commands.rs).

import { invoke } from '@tauri-apps/api/core';

export type AuthMethod =
  | { type: 'password'; password: string }
  | { type: 'key'; identity_id: string; passphrase?: string | null }
  | { type: 'agent' }
  | { type: 'keyboard_interactive'; password: string };

export interface TerminalPrefs {
  term?: string | null;
  font_size?: number | null;
  font_family?: string | null;
  scrollback?: number | null;
  cursor_blink?: boolean | null;
  theme?: string | null;
}

export interface Host {
  id: string;
  label: string;
  address: string;
  port: number;
  username: string;
  auth: AuthMethod;
  group_id?: string | null;
  tags: string[];
  color?: string | null;
  icon?: string | null;
  favorite: boolean;
  notes: string;
  startup_snippet_id?: string | null;
  jump_host_id?: string | null;
  keepalive_secs?: number | null;
  connect_timeout_secs?: number | null;
  env: [string, string][];
  terminal: TerminalPrefs;
  agent_forwarding: boolean;
  compression: boolean;
  os?: string | null;
  host_key_fingerprint?: string | null;
  created_at: string;
  updated_at: string;
  last_used_at?: string | null;
  deleted: boolean;
}

export type SessionStatus = 'connecting' | 'connected' | 'closed' | 'error';

export interface Identity {
  id: string;
  label: string;
  private_key: string;
  passphrase?: string | null;
  kind?: 'ed25519' | 'rsa' | 'ecdsa' | 'imported' | null;
  fingerprint?: string | null;
  created_at: string;
  updated_at: string;
  deleted: boolean;
}

export interface Group {
  id: string;
  label: string;
  parent_id?: string | null;
  color?: string | null;
  icon?: string | null;
  updated_at: string;
  deleted: boolean;
}

export interface VaultSettings {
  theme?: string | null;
  default_term: string;
  font_size: number;
  font_family?: string | null;
  scrollback: number;
  sync_interval_secs: number;
  lock_timeout_secs: number;
  copy_on_select: boolean;
  updated_at?: string | null;
}

export interface Vault {
  schema: number;
  hosts: Host[];
  groups: Group[];
  identities: Identity[];
  snippets: unknown[];
  port_forwards: unknown[];
  settings: VaultSettings;
  updated_at?: string | null;
}

export interface GoogleAccount {
  sub: string;
  email: string;
  name?: string | null;
  picture?: string | null;
}

export interface AppStatus {
  has_vault: boolean;
  unlocked: boolean;
  signed_in: boolean;
  account: GoogleAccount | null;
  device_id: string;
  revision: number;
  vault_path: string;
  key_saved: boolean;
}

export interface VaultPayload {
  vault: Vault;
  revision: number;
}

/** Returned by `session_attach` when a session is still alive (or recently finished). */
export interface SessionAttach {
  found: boolean;
  status: 'connected' | 'closed';
  data_b64: string;
  /** Number of replay bytes, used to drop overlap with the live stream. */
  bytes: number;
}

export const api = {
  appStatus: () => invoke<AppStatus>('app_status'),
  appVersion: () => invoke<string>('app_version'),
  uiSetZoom: (scale: number) => invoke<void>('ui_set_zoom', { scale }),
  uiSetTheme: (theme: string) => invoke<void>('ui_set_theme', { theme }),

  authSignIn: () => invoke<GoogleAccount>('auth_sign_in'),
  authRestore: () => invoke<GoogleAccount | null>('auth_restore'),
  authSignOut: () => invoke<void>('auth_sign_out'),

  vaultCreate: (passphrase: string) => invoke<VaultPayload>('vault_create', { passphrase }),
  vaultUnlock: (passphrase: string) => invoke<VaultPayload>('vault_unlock', { passphrase }),
  vaultAdoptRemote: (passphrase: string) =>
    invoke<VaultPayload>('vault_adopt_remote', { passphrase }),
  vaultAutoUnlock: () => invoke<VaultPayload | null>('vault_try_auto_unlock'),
  vaultLock: () => invoke<void>('vault_lock'),
  vaultGet: () => invoke<VaultPayload | null>('vault_get'),
  vaultSave: (vault: Vault) => invoke<VaultPayload>('vault_save', { vault }),
  vaultSync: () => invoke<VaultPayload>('vault_sync'),

  sshOpen: (sessionId: string, hostId: string, cols: number, rows: number) =>
    invoke<void>('ssh_open', { sessionId, hostId, cols, rows }),
  sshWrite: (sessionId: string, dataB64: string) =>
    invoke<void>('ssh_write', { sessionId, dataB64 }),
  sshResize: (sessionId: string, cols: number, rows: number) =>
    invoke<void>('ssh_resize', { sessionId, cols, rows }),
  sshClose: (sessionId: string) => invoke<void>('ssh_close', { sessionId }),

  localOpen: (sessionId: string, cols: number, rows: number, cwd?: string) =>
    invoke<void>('local_open', { sessionId, cols, rows, cwd: cwd ?? null }),
  localWrite: (sessionId: string, dataB64: string) =>
    invoke<void>('local_write', { sessionId, dataB64 }),
  localResize: (sessionId: string, cols: number, rows: number) =>
    invoke<void>('local_resize', { sessionId, cols, rows }),
  localClose: (sessionId: string) => invoke<void>('local_close', { sessionId }),

  /// One-shot probe: detect the remote OS and persist it on the host.
  hostProbeOs: (hostId: string) => invoke<VaultPayload>('host_probe_os', { hostId }),

  /// Replays an existing session after a webview reload.
  sessionAttach: (sessionId: string) =>
    invoke<SessionAttach | null>('session_attach', { sessionId }),
  sshImportKey: (label: string, pem: string, passphrase: string | null) =>
    invoke<Identity>('ssh_import_key', { label, pem, passphrase }),
  sshKeygen: (kind: string, label: string) =>
    invoke<Identity>('ssh_keygen', { kind, label }),
};

export function errorText(error: unknown): string {
  if (typeof error === 'string') return error;
  if (error instanceof Error) return error.message;
  return String(error);
}

export const newHost = (): Host => ({
  id: crypto.randomUUID(),
  label: '',
  address: '',
  port: 22,
  username: 'root',
  auth: { type: 'password', password: '' },
  tags: [],
  favorite: false,
  notes: '',
  env: [],
  terminal: { font_size: 13, scrollback: 10000 },
  connect_timeout_secs: 15,
  keepalive_secs: 30,
  agent_forwarding: false,
  compression: false,
  host_key_fingerprint: null,
  created_at: new Date().toISOString(),
  updated_at: new Date().toISOString(),
  deleted: false,
});
