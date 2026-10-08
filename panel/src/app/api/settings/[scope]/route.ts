import { adminFetch } from "@/lib/flicksync";
import { hasSession, sameOrigin } from "@/lib/guard";
import { forbidden, invalid, relay, routeNotFound, signedOut } from "@/lib/proxy";
import { readBodyCapped } from "@/lib/read-body";
import { MAX_PATCH_CHARS, isSettingsScope, parsePatchBody } from "@/lib/settings-form";

export const dynamic = "force-dynamic";

type Ctx = { params: Promise<{ scope: string }> };

/** Largest PUT body read, in bytes (64 KiB): never more is held in memory. */
const MAX_PATCH_BYTES = MAX_PATCH_CHARS;

/** The settings of one scope; secrets only as set or not set. Session required. */
export async function GET(_req: Request, ctx: Ctx) {
  if (!(await hasSession())) return signedOut();
  const { scope } = await ctx.params;
  if (!isSettingsScope(scope)) return routeNotFound();
  return relay(await adminFetch(`settings/${scope}`));
}

/** Save changed settings. Session and same-origin required; the body is checked first. */
export async function PUT(req: Request, ctx: Ctx) {
  if (!(await hasSession())) return signedOut();
  if (!(await sameOrigin())) return forbidden();
  const { scope } = await ctx.params;
  if (!isSettingsScope(scope)) return routeNotFound();
  // Content-Length is checked first, then the read stops once 64 KiB are passed.
  const raw = await readBodyCapped(req, MAX_PATCH_BYTES);
  if (raw === null) return invalid("The request is too large. Save fewer changes at once.");
  const parsed = parsePatchBody(raw);
  if (!parsed.ok) return invalid(parsed.message);
  return relay(await adminFetch(`settings/${scope}`, "PUT", { values: parsed.values }));
}
