import {
  issueAccessToken,
  newRefreshToken,
  sha256Hex,
  validateJwtSecret,
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
  /// Comma-separated browser origins allowed to call the API. Desktop builds
  /// use Tauri origins, which never send an Origin header to CORS anyway.
  ALLOWED_ORIGINS?: string;
}

const REFRESH_TTL_DAYS = 60;
const nowIso = () => new Date().toISOString();
const DEFAULT_ALLOWED_ORIGINS = [
  'tauri://localhost',
  'http://tauri.localhost',
  'http://127.0.0.1:1420',
];

/** Best-effort per-isolate throttle. Durable across requests in one isolate,
 *  not globally — sufficient to slow online guessing, not a replacement for
 *  an edge WAF. */
const authAttempts = new Map<string, { count: number; resetAt: number }>();
function rateLimited(ip: string): boolean {
  const now = Date.now();
  const entry = authAttempts.get(ip);
  if (!entry || now >= entry.resetAt) {
    authAttempts.set(ip, { count: 1, resetAt: now + 60_000 });
    return false;
  }
  entry.count += 1;
  return entry.count > 8;
}

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

type BodyResult = { ok: true; value: any } | { ok: false; status: number; error: string };

/** A 200 with a non-JSON body must be a clean failure, not an unhandled 500. */
async function safeJson<T>(res: Response): Promise<T | null> {
  try {
    return (await res.json()) as T;
  } catch {
    return null;
  }
}

/** Vault body limit must fail closed: a malformed env value must never turn
 *  into `NaN` (comparisons with NaN are always false) or a negative limit. */
function vaultMaxBytes(env: Env): number {
  const raw = Number(env.MAX_VAULT_BYTES ?? 8 * 1024 * 1024);
  return Number.isFinite(raw) && raw > 0 ? raw : 8 * 1024 * 1024;
}

/** Reads a JSON body with a real byte limit before parsing (F-19). */
async function readJsonBody(request: Request, maxBytes = 64 * 1024): Promise<BodyResult> {
  const declared = Number(request.headers.get('content-length') ?? '0');
  if (Number.isFinite(declared) && declared > maxBytes) {
    return { ok: false, status: 413, error: 'body_too_large' };
  }
  const text = await request.text();
  if (new TextEncoder().encode(text).byteLength > maxBytes) {
    return { ok: false, status: 413, error: 'body_too_large' };
  }
  try {
    const value = JSON.parse(text);
    if (value === null || typeof value !== 'object' || Array.isArray(value)) {
      return { ok: false, status: 400, error: 'invalid_json' };
    }
    return { ok: true, value };
  } catch {
    return { ok: false, status: 400, error: 'invalid_json' };
  }
}

/** Restrictive CORS: only explicitly configured origins are reflected (F-08). */
function corsHeaders(request: Request, env: Env): Record<string, string> {
  const origin = request.headers.get('origin');
  // R-06: only explicitly configured origins. Deriving one from the request
  // Host made the allowlist attacker-influenceable on setups that let the
  // Host header through. Same-origin page loads never need an ACAO header.
  const allowed = new Set([
    ...DEFAULT_ALLOWED_ORIGINS,
    ...(env.ALLOWED_ORIGINS ?? '').split(',').map(v => v.trim()).filter(Boolean),
  ]);
  const headers: Record<string, string> = {
    'access-control-allow-methods': 'GET,POST,PUT,DELETE,OPTIONS',
    'access-control-allow-headers': 'authorization,content-type',
    vary: 'Origin',
  };
  if (origin && allowed.has(origin)) headers['access-control-allow-origin'] = origin;
  return headers;
}

async function authUser(env: Env, request: Request) {
  const token = bearer(request);
  if (!token) return null;
  const claims = await verifyAccessToken(env.JWT_SECRET, token);
  if (!claims) return null;
  // Access tokens are bearer tokens: check device/user state on every call so
  // logout, device removal and account deletion take effect immediately.
  const device = await env.DB.prepare(
    'SELECT id FROM devices WHERE id = ? AND user_id = ? AND revoked_at IS NULL',
  ).bind(claims.deviceId, claims.userId).first();
  return device ? claims : null;
}

/** Exchange a Google authorization code (loopback redirect + PKCE) for Google identity.
 *  Returns the identity, or a short reason string when Google rejects it. */
async function googleIdentity(env: Env, code: string, codeVerifier: string,
                              redirectUri: string) {
  // Only loopback redirects are valid for the installed app (RFC 8252).
  if (!/^http:\/\/(127\.0\.0\.1|localhost):\d+\//.test(redirectUri)) {
    return 'invalid_redirect_uri';
  }
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
    console.error('google token exchange failed', res.status);
    return 'google_token_exchange_failed'; // never echo upstream details (F-41)
  }
  const tokens = await safeJson<{ id_token?: string }>(res);
  if (!tokens?.id_token) {
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
  const info = await safeJson<{
    sub?: string; email?: string; aud?: string; azp?: string;
    email_verified?: boolean | string; name?: string; picture?: string;
  }>(infoRes);
  if (!info) {
    console.error('google tokeninfo returned a non-JSON body');
    return 'google_identity_rejected';
  }
  const verified = info.email_verified === true || info.email_verified === 'true';
  const audience = info.azp ?? info.aud;
  if (!verified || audience !== env.GOOGLE_CLIENT_ID || !info.sub || !info.email) {
    console.error('google tokeninfo: unverified email or audience mismatch');
    return 'google_identity_rejected';
  }
  return { sub: info.sub, email: info.email, name: info.name ?? '', picture: info.picture ?? '' };
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
  const parsed = await readJsonBody(request);
  if (!parsed.ok) return err(parsed.status, parsed.error);
  input = parsed.value;
  if (!input.code || !input.code_verifier || !input.redirect_uri) {
    return err(400, 'missing_code_verifier');
  }

  const identity = await googleIdentity(env, input.code, input.code_verifier, input.redirect_uri);
  if (typeof identity === 'string') return err(401, 'google_auth_failed');

  interface UserRow {
    id: string;
    email: string;
    name: string | null;
    picture: string | null;
    google_sub: string;
  }
  let user = await env.DB.prepare(
    'SELECT id, email, name, picture, google_sub FROM users WHERE google_sub = ?',
  ).bind(identity.sub).first<UserRow>();
  if (!user) {
    const id = crypto.randomUUID();
    await env.DB.prepare(
      'INSERT INTO users (id, google_sub, email, name, picture) VALUES (?, ?, ?, ?, ?)',
    ).bind(id, identity.sub, identity.email, identity.name, identity.picture).run();
    user = { id, email: identity.email, name: identity.name, picture: identity.picture, google_sub: identity.sub };
  } else {
    await env.DB.prepare(
      'UPDATE users SET email = ?, name = ?, picture = ?, updated_at = ? WHERE id = ?',
    ).bind(identity.email, identity.name, identity.picture, nowIso(), user.id).run();
    user = { ...user, email: identity.email, name: identity.name, picture: identity.picture };
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
  const familyId = crypto.randomUUID();
  const expires = new Date(Date.now() + REFRESH_TTL_DAYS * 86400_000).toISOString();
  await env.DB.prepare(
    `INSERT INTO refresh_tokens (id, user_id, device_id, token_hash, expires_at, family_id)
     VALUES (?, ?, ?, ?, ?, ?)`,
  ).bind(crypto.randomUUID(), user.id, deviceId, refreshHash, expires, familyId).run();

  const { token, expiresIn } = await issueAccessToken(env.JWT_SECRET, user.id, deviceId);
  return json({
    access_token: token,
    refresh_token: refreshToken,
    expires_in: expiresIn,
    device_id: deviceId,
    user: { email: user.email, name: user.name ?? '', picture: user.picture ?? '', sub: user.google_sub },
    vault: { revision: await vaultRevision(env, user.id) },
  });
}

async function revokeFamily(env: Env, familyId: string, reason: string) {
  const stamp = nowIso();
  await env.DB.batch([
    env.DB.prepare(
      `UPDATE refresh_tokens SET revoked_at = ?, revoked_reason = ?
       WHERE family_id = ? AND revoked_at IS NULL`,
    ).bind(stamp, reason, familyId),
    env.DB.prepare(
      `UPDATE devices SET revoked_at = ? WHERE id IN
       (SELECT device_id FROM refresh_tokens WHERE family_id = ?)`,
    ).bind(stamp, familyId),
  ]);
}

async function authRefresh(env: Env, request: Request): Promise<Response> {
  const parsed = await readJsonBody(request);
  if (!parsed.ok) return err(parsed.status, parsed.error);
  const refreshToken = parsed.value.refresh_token;
  if (typeof refreshToken !== 'string' || !refreshToken) return err(400, 'missing_refresh_token');

  const hash = await sha256Hex(refreshToken);
  interface TokenRow {
    id: string; user_id: string; device_id: string; family_id: string;
  }
  const row = await env.DB.prepare(
    `SELECT id, user_id, device_id, family_id FROM refresh_tokens
     WHERE token_hash = ? AND revoked_at IS NULL AND expires_at > ?`,
  ).bind(hash, nowIso()).first<TokenRow>();

  if (!row) {
    // A previously rotated token being replayed means the token was stolen:
    // revoke the entire family and the device (F-09).
    const reused = await env.DB.prepare(
      'SELECT family_id FROM refresh_tokens WHERE token_hash = ? AND revoked_at IS NOT NULL',
    ).bind(hash).first<{ family_id: string }>();
    if (reused?.family_id) await revokeFamily(env, reused.family_id, 'reuse_detected');
    return err(401, 'invalid_refresh_token');
  }

  const device = await env.DB.prepare(
    'SELECT id FROM devices WHERE id = ? AND user_id = ? AND revoked_at IS NULL',
  ).bind(row.device_id, row.user_id).first();
  if (!device) return err(401, 'invalid_refresh_token');

  const next = newRefreshToken();
  const nextHash = await sha256Hex(next);
  const expires = new Date(Date.now() + REFRESH_TTL_DAYS * 86400_000).toISOString();
  const stamp = nowIso();
  // F-09: claim the old token FIRST with a conditional UPDATE. Only the
  // request that actually flips `revoked_at` is allowed to insert its successor.
  // Putting the INSERT in the same D1 batch left an orphaned valid token when
  // the losing concurrent request returned 401.
  const claimed = await env.DB.prepare(
    `UPDATE refresh_tokens SET revoked_at = ?, replaced_by = ?
     WHERE id = ? AND token_hash = ? AND revoked_at IS NULL`,
  ).bind(stamp, nextHash, row.id, hash).run();
  if ((claimed.meta?.changes ?? 0) !== 1) {
    // Another refresh already rotated this token — fail closed, issue nothing.
    return err(401, 'invalid_refresh_token');
  }
  await env.DB.batch([
    env.DB.prepare(
      `INSERT INTO refresh_tokens (id, user_id, device_id, token_hash, expires_at, family_id)
       VALUES (?, ?, ?, ?, ?, ?)`,
    ).bind(crypto.randomUUID(), row.user_id, row.device_id, nextHash, expires, row.family_id),
    env.DB.prepare('UPDATE devices SET last_seen_at = ? WHERE id = ?')
      .bind(stamp, row.device_id),
  ]);

  const user = await env.DB.prepare(
    'SELECT email, name, picture, google_sub FROM users WHERE id = ?',
  ).bind(row.user_id).first<{ email: string; name: string | null; picture: string | null; google_sub: string }>();
  const { token, expiresIn } = await issueAccessToken(env.JWT_SECRET, row.user_id, row.device_id);
  return json({
    access_token: token,
    refresh_token: next,
    expires_in: expiresIn,
    device_id: row.device_id,
    user: {
      email: user?.email ?? '',
      name: user?.name ?? '',
      picture: user?.picture ?? '',
      sub: user?.google_sub ?? '',
    },
    vault: { revision: await vaultRevision(env, row.user_id) },
  });
}

async function authLogout(env: Env, request: Request): Promise<Response> {
  // Require the bearer token: a stolen refresh token alone must not let an
  // attacker force a victim offline (F-25).
  const auth = await authUser(env, request);
  if (!auth) return err(401, 'unauthorized');
  const parsed = await readJsonBody(request);
  if (parsed.ok && typeof parsed.value.refresh_token === 'string') {
    const hash = await sha256Hex(parsed.value.refresh_token);
    const row = await env.DB.prepare(
      `SELECT family_id FROM refresh_tokens
       WHERE token_hash = ? AND user_id = ? AND device_id = ?`,
    ).bind(hash, auth.userId, auth.deviceId).first<{ family_id: string }>();
    // Only revoke the family that belongs to the authenticated device. Without
    // the device_id check, a token from device A paired with an access token
    // from device B could revoke A's whole session family (cross-device DoS).
    if (row?.family_id) await revokeFamily(env, row.family_id, 'logout');
  }
  // Revoke the device so already-issued access tokens stop working (F-22).
  await env.DB.prepare('UPDATE devices SET revoked_at = ? WHERE id = ? AND user_id = ?')
    .bind(nowIso(), auth.deviceId, auth.userId).run();
  return json({ ok: true });
}

async function me(env: Env, request: Request): Promise<Response> {
  const auth = await authUser(env, request);
  if (!auth) return err(401, 'unauthorized');
  const user = await env.DB.prepare(
    'SELECT email, name, picture, google_sub FROM users WHERE id = ?',
  ).bind(auth.userId).first<{ email: string; name: string | null; picture: string | null; google_sub: string }>();
  if (!user) return err(404, 'user_not_found');
  return json({
    user: { email: user.email, name: user.name ?? '', picture: user.picture ?? '', sub: user.google_sub },
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

  const maxBytes = vaultMaxBytes(env);
  const parsed = await readJsonBody(request, maxBytes + 64 * 1024);
  if (!parsed.ok) return err(parsed.status, parsed.error);
  const body = parsed.value;
  if (typeof body.base_revision !== 'number' || typeof body.blob !== 'string') {
    return err(400, 'missing_base_revision_or_blob');
  }
  if (new TextEncoder().encode(body.blob).byteLength > maxBytes) {
    return err(413, 'vault_too_large');
  }

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
  if (id === auth.deviceId) return err(400, 'cannot_delete_current_device');
  const owned = await env.DB.prepare('SELECT id FROM devices WHERE id = ? AND user_id = ?')
    .bind(id, auth.userId).first();
  if (!owned) return err(404, 'device_not_found');
  // Mark revoked (do not delete): verifyAccessToken must reject this device on
  // every subsequent API call, even with an unexpired access token.
  await env.DB.batch([
    env.DB.prepare('UPDATE devices SET revoked_at = ? WHERE id = ?').bind(nowIso(), id),
    env.DB.prepare('UPDATE refresh_tokens SET revoked_at = ?, revoked_reason = ? WHERE device_id = ? AND revoked_at IS NULL')
      .bind(nowIso(), 'device_removed', id),
  ]);
  return json({ ok: true });
}

async function deleteAccount(env: Env, request: Request): Promise<Response> {
  const auth = await authUser(env, request);
  if (!auth) return err(401, 'unauthorized');
  await env.DB.prepare('DELETE FROM users WHERE id = ?').bind(auth.userId).run(); // cascades all
  return json({ ok: true });
}

// ------------------------------------------------------------ router

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const cors = corsHeaders(request, env);
    if (request.method === 'OPTIONS') return new Response(null, { status: 204, headers: cors });
    const url = new URL(request.url);
    const path = url.pathname.replace(/\/+$/, '') || '/';
    const method = request.method;

    // `cf-connecting-ip` is set by Cloudflare and cannot be forged by the
    // client. Never fall back to `x-forwarded-for` here: an attacker could
    // rotate that header and get a fresh rate-limit bucket (F-14).
    const ip = request.headers.get('cf-connecting-ip') ?? 'local';
    if (path.startsWith('/v1/auth/') && rateLimited(ip)) {
      return json({ error: 'rate_limited' }, 429, cors);
    }

    // Refuse to serve any API route with an unset or publicly known secret.
    if (path.startsWith('/v1/')) {
      try {
        validateJwtSecret(env.JWT_SECRET);
      } catch {
        console.error('JWT_SECRET is missing or too weak; refusing to serve /v1/*');
        return err(500, 'server_misconfigured');
      }
    }

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
      const raw = path.slice('/v1/devices/'.length);
      let deviceId = raw;
      try {
        deviceId = decodeURIComponent(raw);
      } catch {
        deviceId = ''; // malformed percent-escape
      }
      response = deviceId
        ? await deleteDevice(env, request, deviceId)
        : err(400, 'invalid_device_id');
    } else if (method === 'DELETE' && path === '/v1/account') response = await deleteAccount(env, request);
    else response = err(404, 'not_found');

    // Attach restrictive CORS to every response.
    const headers = new Headers(response.headers);
    for (const [k, v] of Object.entries(cors)) headers.set(k, v);
    return new Response(response.body, { status: response.status, headers });
  },
};
