import { SignJWT, jwtVerify } from 'jose';

export interface AccessClaims {
  userId: string;
  deviceId: string;
}

const ISSUER = 'rterm-bridge';
const AUDIENCE = 'rterm-app';
const ACCESS_TTL_SECS = 3600;
const MIN_SECRET_BYTES = 32;
const WEAK_MARKERS = [
  'dev-only', 'dev_only', 'changeme', 'change-me', 'change_me',
  'example', 'placeholder', 'replace',
];

/** Fail closed: a missing/known-weak secret must never sign a real token.
 *  HMAC-SHA256 is safe with any high-entropy >=32-byte key, so no extra KDF
 *  is needed — the important part is refusing predictable values. */
export function validateJwtSecret(secret: string | undefined | null): string {
  if (typeof secret !== 'string') throw new Error('JWT_SECRET is not set');
  const value = secret.trim();
  if (value.length < MIN_SECRET_BYTES) {
    throw new Error(`JWT_SECRET must be at least ${MIN_SECRET_BYTES} bytes`);
  }
  const lower = value.toLowerCase();
  if (WEAK_MARKERS.some(marker => lower.includes(marker))) {
    throw new Error('JWT_SECRET looks like a placeholder');
  }
  if (new Set(value).size < 8) throw new Error('JWT_SECRET has too little entropy');
  return value;
}

function keyFor(secret: string): Uint8Array {
  return new TextEncoder().encode(validateJwtSecret(secret));
}

export async function issueAccessToken(
  secret: string,
  userId: string,
  deviceId: string,
): Promise<{ token: string; expiresIn: number }> {
  const key = keyFor(secret);
  const token = await new SignJWT({ did: deviceId })
    .setProtectedHeader({ alg: 'HS256' })
    .setSubject(userId)
    .setIssuer(ISSUER)
    .setAudience(AUDIENCE)
    .setIssuedAt()
    .setExpirationTime(`${ACCESS_TTL_SECS}s`)
    .sign(key);
  return { token, expiresIn: ACCESS_TTL_SECS };
}

export async function verifyAccessToken(
  secret: string,
  token: string,
): Promise<AccessClaims | null> {
  try {
    const key = keyFor(secret);
    const { payload } = await jwtVerify(token, key, {
      issuer: ISSUER,
      audience: AUDIENCE,
    });
    if (!payload.sub || typeof payload.did !== 'string') return null;
    return { userId: payload.sub, deviceId: payload.did };
  } catch {
    return null;
  }
}

/** Long random refresh token; only its SHA-256 hash is ever stored. */
export function newRefreshToken(): string {
  const bytes = new Uint8Array(32);
  crypto.getRandomValues(bytes);
  return base64url(bytes);
}

export async function sha256Hex(text: string): Promise<string> {
  const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(text));
  return [...new Uint8Array(digest)].map(b => b.toString(16).padStart(2, '0')).join('');
}

function base64url(bytes: Uint8Array): string {
  let out = '';
  for (const b of bytes) out += String.fromCharCode(b);
  return btoa(out).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}
