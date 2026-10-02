// Stateless signed session cookie. No store, nothing to persist: the signing key is derived from
// the panel password, so changing the password logs everyone out.
import { createHash, createHmac, randomBytes, timingSafeEqual } from "node:crypto";

export const SESSION_COOKIE = "flick_panel_session";
export const SESSION_TTL_SECS = 12 * 3600;

function key(password: string): Buffer {
  return createHmac("sha256", password).update("flick-panel/session/v1").digest();
}

function sign(password: string, payload: string): string {
  return createHmac("sha256", key(password)).update(payload).digest("base64url");
}

export function createSessionToken(
  password: string,
  nowMs: number = Date.now(),
  ttlSecs: number = SESSION_TTL_SECS,
): string {
  const exp = Math.floor(nowMs / 1000) + ttlSecs;
  const payload = `${exp}.${randomBytes(8).toString("base64url")}`;
  return `${payload}.${sign(password, payload)}`;
}

export function verifySessionToken(
  password: string,
  token: string | undefined,
  nowMs: number = Date.now(),
): boolean {
  if (!password || !token) return false;
  const parts = token.split(".");
  if (parts.length !== 3) return false;
  const [expRaw, nonce, sig] = parts;
  const payload = `${expRaw}.${nonce}`;
  const expected = Buffer.from(sign(password, payload));
  const given = Buffer.from(sig);
  if (expected.length !== given.length || !timingSafeEqual(expected, given)) return false;
  const exp = Number(expRaw);
  return Number.isFinite(exp) && exp * 1000 > nowMs;
}

/** Constant-time password check (hashes first so lengths do not leak). */
export function passwordMatches(input: string, expected: string): boolean {
  const a = createHash("sha256").update(input).digest();
  const b = createHash("sha256").update(expected).digest();
  return timingSafeEqual(a, b);
}
