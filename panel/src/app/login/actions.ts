"use server";

import { cookies, headers } from "next/headers";
import { redirect } from "next/navigation";

import { panelConfig } from "@/lib/config";
import { AttemptLimiter } from "@/lib/limiter";
import {
  SESSION_COOKIE,
  SESSION_TTL_SECS,
  createSessionToken,
  passwordMatches,
} from "@/lib/session";

const limiter = new AttemptLimiter(5, 15 * 60 * 1000);

export interface LoginState {
  error?: string;
}

export async function login(_prev: LoginState, form: FormData): Promise<LoginState> {
  if (!panelConfig.password) {
    return { error: "PANEL_PASSWORD is not set. Add it to .env and restart the panel." };
  }
  const h = await headers();
  const client = h.get("x-forwarded-for")?.split(",")[0]?.trim() || h.get("x-real-ip") || "local";

  const wait = limiter.retryAfterSecs(client);
  if (wait > 0) {
    return { error: `Too many attempts. Try again in ${Math.ceil(wait / 60)} min.` };
  }

  const given = String(form.get("password") ?? "");
  if (!passwordMatches(given, panelConfig.password)) {
    limiter.fail(client);
    return { error: "That password is not right." };
  }
  limiter.success(client);

  const jar = await cookies();
  jar.set(SESSION_COOKIE, createSessionToken(panelConfig.password), {
    httpOnly: true,
    sameSite: "strict",
    // Behind a TLS-terminating proxy the panel itself speaks http; trust its header.
    secure: h.get("x-forwarded-proto") === "https",
    path: "/",
    maxAge: SESSION_TTL_SECS,
  });
  redirect("/");
}

export async function logout(): Promise<void> {
  const jar = await cookies();
  jar.delete(SESSION_COOKIE);
  redirect("/login");
}
