import { NextResponse } from "next/server";

import { ADMIN_RESOURCES, type AdminResource, adminFetch } from "@/lib/flicksync";
import { hasSession } from "@/lib/guard";

export const dynamic = "force-dynamic";

/** Read-only proxy to FlickSync's admin API for the page's polling. Session required. */
export async function GET(_req: Request, ctx: { params: Promise<{ resource: string }> }) {
  if (!(await hasSession())) {
    return NextResponse.json({ error: { code: "SIGNED_OUT" } }, { status: 401 });
  }
  const { resource } = await ctx.params;
  if (!(ADMIN_RESOURCES as readonly string[]).includes(resource)) {
    return NextResponse.json({ error: { code: "NOT_FOUND" } }, { status: 404 });
  }
  const res = await adminFetch(resource as AdminResource);
  return NextResponse.json(res.body, { status: res.status, headers: { "Cache-Control": "no-store" } });
}
