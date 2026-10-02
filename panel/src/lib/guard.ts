import "server-only";

import { cookies, headers } from "next/headers";

import { panelConfig } from "./config";
import { SESSION_COOKIE, verifySessionToken } from "./session";

export async function hasSession(): Promise<boolean> {
  const jar = await cookies();
  return verifySessionToken(panelConfig.password, jar.get(SESSION_COOKIE)?.value);
}

/** Cookies are SameSite=Strict; this also refuses a mutation whose Origin is another site. */
export async function sameOrigin(): Promise<boolean> {
  const h = await headers();
  const origin = h.get("origin");
  if (!origin) return true;
  try {
    return new URL(origin).host === h.get("host");
  } catch {
    return false;
  }
}
