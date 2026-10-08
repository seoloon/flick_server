import { adminFetch } from "@/lib/flicksync";
import { hasSession, sameOrigin } from "@/lib/guard";
import { forbidden, relay, routeNotFound, signedOut } from "@/lib/proxy";

export const dynamic = "force-dynamic";

/** Apply the stored server settings. Only the server scope can be applied this way. */
export async function POST(_req: Request, ctx: { params: Promise<{ scope: string }> }) {
  if (!(await hasSession())) return signedOut();
  if (!(await sameOrigin())) return forbidden();
  const { scope } = await ctx.params;
  if (scope !== "server") return routeNotFound();
  return relay(await adminFetch("settings/server/reload", "POST"));
}
