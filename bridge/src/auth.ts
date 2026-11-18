import { SignJWT, jwtVerify } from 'jose';

export interface AccessClaims {
  userId: string;
  deviceId: string;
}

const ISSUER = 'rterm-bridge';
const AUDIENCE = 'rterm-app';
const ACCESS_TTL_SECS = 3600;

export async function issueAccessToken(
  secret: string,
  userId: string,
  deviceId: string,
): Promise<{ token: string; expiresIn: number }> {
  const key = new TextEncoder().encode(secret);
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
    const key = new TextEncoder().encode(secret);
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
