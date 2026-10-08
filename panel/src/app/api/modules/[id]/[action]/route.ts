import { adminFetch } from "@/lib/flicksync";
import { hasSession, sameOrigin } from "@/lib/guard";
import { isModuleAction, isModuleId } from "@/lib/module-status";
import { forbidden, relay, routeNotFound, signedOut } from "@/lib/proxy";

export const dynamic = "force-dynamic";

/** Start, stop or reload a module. Session and same-origin required. */
export async function POST(_req: Request, ctx: { params: Promise<{ id: string; action: string }> }) {
  if (!(await hasSession())) return signedOut();
  if (!(await sameOrigin())) return forbidden();
  const { id, action } = await ctx.params;
  if (!isModuleId(id) || !isModuleAction(action)) return routeNotFound();
  return relay(await adminFetch(`modules/${id}/${action}`, "POST"));
}
