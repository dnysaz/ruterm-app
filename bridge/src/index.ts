import {
  issueAccessToken,
  newRefreshToken,
  sha256Hex,
  verifyAccessToken,
} from './auth';
import { landing, privacy } from './pages';

export interface Env {
  DB: D1Database;
  GOOGLE_CLIENT_ID: string;
  /// Google requires the Desktop client secret at the token endpoint
  /// (it is not treated as confidential for installed apps, PKCE still
  /// applies). Kept in the Worker, never in the app or the repo.
  GOOGLE_CLIENT_SECRET?: string;
  MAX_VAULT_BYTES?: string;
  JWT_SECRET: string; // wrangler secret put JWT_SECRET
}

const REFRESH_TTL_DAYS = 60;
const nowIso = () => new Date().toISOString();

// ------------------------------------------------------------ helpers

function json(data: unknown, status = 200, extra?: Record<string, string>): Response {
  return new Response(JSON.stringify(data), {
    status,
    headers: { 'content-type': 'application/json; charset=utf-8', ...extra },
  });
}

function err(status: number, message: string): Response {
  return json({ error: message }, status);
}

function bearer(request: Request): string | null {
  const value = request.headers.get('authorization') ?? '';
  const match = /^Bearer\s+(.+)$/i.exec(value.trim());
  return match?.[1] ?? null;
}

async function authUser(env: Env, request: Request) {
  const token = bearer(request);
  if (!token) return null;
  return verifyAccessToken(env.JWT_SECRET, token);
}

/** Exchange a Google authorization code (loopback redirect + PKCE) for Google identity.
 *  Returns the identity, or a short reason string when Google rejects it. */
async function googleIdentity(env: Env, code: string, codeVerifier: string,
                              redirectUri: string) {
  const body = new URLSearchParams({
    grant_type: 'authorization_code',
    code,
    client_id: env.GOOGLE_CLIENT_ID,
    code_verifier: codeVerifier,
    redirect_uri: redirectUri,
  });
  if (env.GOOGLE_CLIENT_SECRET) body.set('client_secret', env.GOOGLE_CLIENT_SECRET);
  const res = await fetch('https://oauth2.googleapis.com/token', {
    method: 'POST',
    headers: { 'content-type': 'application/x-www-form-urlencoded' },
    body,
  });
  if (!res.ok) {
    const detail = await res.text();
    console.error('google token exchange failed', res.status, detail);
    return `google token endpoint ${res.status}: ${detail.slice(0, 200)}`;
  }
  const tokens = (await res.json()) as { id_token?: string };
  if (!tokens.id_token) {
    console.error('google token endpoint returned no id_token');
    return 'google token endpoint returned no id_token';
  }

  // tokeninfo validates signature, exp and issuer server-side (v1).
  const infoRes = await fetch(
    `https://oauth2.googleapis.com/tokeninfo?id_token=${encodeURIComponent(tokens.id_token)}`,
  );
  if (!infoRes.ok) {
    console.error('google tokeninfo failed', infoRes.status);
    return `google tokeninfo ${infoRes.status}`;
  }
  const info = (await infoRes.json()) as {
    sub?: string; email?: string; aud?: string; name?: string;
  };
  if (info.aud !== env.GOOGLE_CLIENT_ID || !info.sub || !info.email) {
    console.error('google tokeninfo unexpected content', JSON.stringify(info).slice(0, 300));
    return 'google tokeninfo aud/sub/email mismatch';
  }
  return { sub: info.sub, email: info.email, name: info.name ?? '' };
}

async function vaultRevision(env: Env, userId: string): Promise<number> {
  const row = await env.DB.prepare(
    'SELECT revision FROM vaults WHERE user_id = ?',
  ).bind(userId).first<{ revision: number }>();
  return row?.revision ?? 0;
}

// ------------------------------------------------------------ routes

async function authGoogle(env: Env, request: Request): Promise<Response> {
  let input: {
    code?: string; code_verifier?: string; redirect_uri?: string;
    device?: { name?: string; platform?: string; app_version?: string };
  };
  try {
    input = await request.json();
  } catch {
    return err(400, 'invalid_json');
  }
  if (!input.code || !input.code_verifier || !input.redirect_uri) {
    return err(400, 'missing_code_verifier');
  }

  const identity = await googleIdentity(env, input.code, input.code_verifier, input.redirect_uri);
  if (typeof identity === 'string') return err(401, `google_auth_failed: ${identity}`);

  let user = await env.DB.prepare('SELECT id, email FROM users WHERE google_sub = ?')
    .bind(identity.sub).first<{ id: string; email: string }>();
  if (!user) {
    const id = crypto.randomUUID();
    await env.DB.prepare(
      'INSERT INTO users (id, google_sub, email, name) VALUES (?, ?, ?, ?)',
    ).bind(id, identity.sub, identity.email, identity.name).run();
    user = { id, email: identity.email };
  } else {
    await env.DB.prepare(
      'UPDATE users SET email = ?, name = ?, updated_at = ? WHERE id = ?',
    ).bind(identity.email, identity.name, nowIso(), user.id).run();
  }

  const device = input.device ?? {};
  const deviceId = crypto.randomUUID();
  await env.DB.prepare(
    `INSERT INTO devices (id, user_id, name, platform, app_version) VALUES (?, ?, ?, ?, ?)`,
  ).bind(
    deviceId,
    user.id,
    (device.name ?? 'Unknown device').slice(0, 120),
    (device.platform ?? 'unknown').slice(0, 40),
    (device.app_version ?? '').slice(0, 40),
  ).run();

  const refreshToken = newRefreshToken();
  const refreshHash = await sha256Hex(refreshToken);
  const expires = new Date(Date.now() + REFRESH_TTL_DAYS * 86400_000).toISOString();
  await env.DB.prepare(
    'INSERT INTO refresh_tokens (id, user_id, device_id, token_hash, expires_at) VALUES (?, ?, ?, ?, ?)',
  ).bind(crypto.randomUUID(), user.id, deviceId, refreshHash, expires).run();

  const { token, expiresIn } = await issueAccessToken(env.JWT_SECRET, user.id, deviceId);
  return json({
    access_token: token,
    refresh_token: refreshToken,
    expires_in: expiresIn,
    device_id: deviceId,
    user: { email: identity.email, name: identity.name },
    vault: { revision: await vaultRevision(env, user.id) },
  });
}

async function authRefresh(env: Env, request: Request): Promise<Response> {
  let body: { refresh_token?: string };
  try {
    body = await request.json();
  } catch {
    return err(400, 'invalid_json');
  }
  if (!body.refresh_token) return err(400, 'missing_refresh_token');

  const hash = await sha256Hex(body.refresh_token);
  const row = await env.DB.prepare(
    `SELECT token_hash, user_id, device_id FROM refresh_tokens
     WHERE token_hash = ? AND revoked_at IS NULL AND expires_at > ?`,
  ).bind(hash, nowIso()).first<{ token_hash: string; user_id: string; device_id: string }>();
  if (!row) return err(401, 'invalid_refresh_token');

  // Rotate: revoke the presented token, issue a fresh pair.
  await env.DB.prepare('UPDATE refresh_tokens SET revoked_at = ? WHERE token_hash = ?')
    .bind(nowIso(), hash).run();
  const next = newRefreshToken();
  const nextHash = await sha256Hex(next);
  const expires = new Date(Date.now() + REFRESH_TTL_DAYS * 86400_000).toISOString();
  await env.DB.prepare(
    'INSERT INTO refresh_tokens (id, user_id, device_id, token_hash, expires_at) VALUES (?, ?, ?, ?, ?)',
  ).bind(crypto.randomUUID(), row.user_id, row.device_id, nextHash, expires).run();
  await env.DB.prepare('UPDATE devices SET last_seen_at = ? WHERE id = ?')
    .bind(nowIso(), row.device_id).run();

  const user = await env.DB.prepare('SELECT email, name FROM users WHERE id = ?')
    .bind(row.user_id).first<{ email: string; name: string }>();
  const { token, expiresIn } = await issueAccessToken(env.JWT_SECRET, row.user_id, row.device_id);
  return json({
    access_token: token,
    refresh_token: next,
    expires_in: expiresIn,
    device_id: row.device_id,
    user: { email: user?.email ?? '', name: user?.name ?? '' },
    vault: { revision: await vaultRevision(env, row.user_id) },
  });
}

async function authLogout(env: Env, request: Request): Promise<Response> {
  let body: { refresh_token?: string };
  try {
    body = await request.json();
  } catch {
    body = {};
  }
  if (body.refresh_token) {
    const hash = await sha256Hex(body.refresh_token);
    await env.DB.prepare('UPDATE refresh_tokens SET revoked_at = ? WHERE token_hash = ?')
      .bind(nowIso(), hash).run();
  }
  return json({ ok: true });
}

async function me(env: Env, request: Request): Promise<Response> {
  const auth = await authUser(env, request);
  if (!auth) return err(401, 'unauthorized');
  const user = await env.DB.prepare('SELECT email, name FROM users WHERE id = ?')
    .bind(auth.userId).first<{ email: string; name: string }>();
  if (!user) return err(404, 'user_not_found');
  return json({
    user: { email: user.email, name: user.name },
    device_id: auth.deviceId,
    vault: { revision: await vaultRevision(env, auth.userId) },
  });
}

async function getVault(env: Env, request: Request): Promise<Response> {
  const auth = await authUser(env, request);
  if (!auth) return err(401, 'unauthorized');
  const row = await env.DB.prepare(
    'SELECT revision, blob, updated_at FROM vaults WHERE user_id = ?',
  ).bind(auth.userId).first<{ revision: number; blob: string; updated_at: string }>();
  return json({ revision: row?.revision ?? 0, blob: row?.blob ?? '', updated_at: row?.updated_at ?? null });
}

async function putVault(env: Env, request: Request): Promise<Response> {
  const auth = await authUser(env, request);
  if (!auth) return err(401, 'unauthorized');

  let body: { base_revision?: number; blob?: string };
  try {
    body = await request.json();
  } catch {
    return err(400, 'invalid_json');
  }
  if (typeof body.base_revision !== 'number' || typeof body.blob !== 'string') {
    return err(400, 'missing_base_revision_or_blob');
  }
  const maxBytes = Number(env.MAX_VAULT_BYTES ?? 8 * 1024 * 1024);
  if (body.blob.length > maxBytes) return err(413, 'vault_too_large');

  const current = await env.DB.prepare('SELECT revision FROM vaults WHERE user_id = ?')
    .bind(auth.userId).first<{ revision: number }>();
  const currentRevision = current?.revision ?? 0;

  if (current && body.base_revision !== currentRevision) {
    const row = await env.DB.prepare(
      'SELECT revision, blob, updated_at FROM vaults WHERE user_id = ?',
    ).bind(auth.userId).first();
    return json({ error: 'revision_conflict', current: row }, 409);
  }

  if (!current && body.base_revision !== 0) {
    return err(409, 'revision_conflict');
  }

  if (!current) {
    await env.DB.prepare(
      'INSERT INTO vaults (user_id, revision, blob) VALUES (?, 1, ?)',
    ).bind(auth.userId, body.blob).run();
    return json({ revision: 1 });
  }

  const next = currentRevision + 1;
  const updated = await env.DB.prepare(
    'UPDATE vaults SET revision = ?, blob = ?, updated_at = ? WHERE user_id = ? AND revision = ?',
  ).bind(next, body.blob, nowIso(), auth.userId, currentRevision).run();
  if ((updated.meta?.changes ?? 0) === 0) {
    const row = await env.DB.prepare(
      'SELECT revision, blob, updated_at FROM vaults WHERE user_id = ?',
    ).bind(auth.userId).first();
    return json({ error: 'revision_conflict', current: row }, 409);
  }
  return json({ revision: next });
}

async function listDevices(env: Env, request: Request): Promise<Response> {
  const auth = await authUser(env, request);
  if (!auth) return err(401, 'unauthorized');
  const rows = await env.DB.prepare(
    `SELECT id, name, platform, app_version, created_at, last_seen_at FROM devices
     WHERE user_id = ? ORDER BY last_seen_at DESC`,
  ).bind(auth.userId).all();
  return json({
    devices: (rows.results ?? []).map((d: Record<string, unknown>) => ({
      ...d,
      current: d.id === auth.deviceId,
    })),
  });
}

async function deleteDevice(env: Env, request: Request, id: string): Promise<Response> {
  const auth = await authUser(env, request);
  if (!auth) return err(401, 'unauthorized');
  const owned = await env.DB.prepare('SELECT id FROM devices WHERE id = ? AND user_id = ?')
    .bind(id, auth.userId).first();
  if (!owned) return err(404, 'device_not_found');
  await env.DB.prepare('DELETE FROM devices WHERE id = ?').bind(id).run(); // cascades refresh tokens
  return json({ ok: true });
}

async function deleteAccount(env: Env, request: Request): Promise<Response> {
  const auth = await authUser(env, request);
  if (!auth) return err(401, 'unauthorized');
  await env.DB.prepare('DELETE FROM users WHERE id = ?').bind(auth.userId).run(); // cascades all
  return json({ ok: true });
}

// ------------------------------------------------------------ router

const CORS: Record<string, string> = {
  'access-control-allow-origin': '*',
  'access-control-allow-methods': 'GET,POST,DELETE,OPTIONS',
  'access-control-allow-headers': 'authorization,content-type',
};

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    if (request.method === 'OPTIONS') return new Response(null, { status: 204, headers: CORS });
    const url = new URL(request.url);
    const path = url.pathname.replace(/\/+$/, '') || '/';
    const method = request.method;

    let response: Response;
    if (method === 'GET' && path === '/') response = landing();
    else if (method === 'GET' && path === '/privacy') response = privacy();
    else if (method === 'GET' && path === '/health') response = json({ ok: true, service: 'rterm-bridge' });
    else if (method === 'POST' && path === '/v1/auth/google') response = await authGoogle(env, request);
    else if (method === 'POST' && path === '/v1/auth/refresh') response = await authRefresh(env, request);
    else if (method === 'POST' && path === '/v1/auth/logout') response = await authLogout(env, request);
    else if (method === 'GET' && path === '/v1/me') response = await me(env, request);
    else if (method === 'GET' && path === '/v1/vault') response = await getVault(env, request);
    else if (method === 'PUT' && path === '/v1/vault') response = await putVault(env, request);
    else if (method === 'GET' && path === '/v1/devices') response = await listDevices(env, request);
    else if (method === 'DELETE' && path.startsWith('/v1/devices/')) {
      response = await deleteDevice(env, request, decodeURIComponent(path.slice('/v1/devices/'.length)));
    } else if (method === 'DELETE' && path === '/v1/account') response = await deleteAccount(env, request);
    else response = err(404, 'not_found');

    // Attach CORS to API responses.
    const headers = new Headers(response.headers);
    for (const [k, v] of Object.entries(CORS)) headers.set(k, v);
    return new Response(response.body, { status: response.status, headers });
  },
};
