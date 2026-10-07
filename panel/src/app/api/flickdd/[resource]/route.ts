import { NextResponse } from "next/server";

import { DD_RESOURCES, ddFetch } from "@/lib/flicksync";
import { hasSession } from "@/lib/guard";

export const dynamic = "force-dynamic";

/** Read-only proxy to FlickDD's admin routes for the page's polling. Session required. */
export async function GET(_req: Request, ctx: { params: Promise<{ resource: string }> }) {
  if (!(await hasSession())) {
    return NextResponse.json({ error: { code: "SIGNED_OUT" } }, { status: 401 });
  }
  const { resource } = await ctx.params;
  if (!(DD_RESOURCES as readonly string[]).includes(resource)) {
    return NextResponse.json({ error: { code: "NOT_FOUND" } }, { status: 404 });
  }
  const res = await ddFetch(resource);
  return NextResponse.json(res.body, { status: res.status, headers: { "Cache-Control": "no-store" } });
}
