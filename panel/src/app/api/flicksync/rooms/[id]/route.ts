import { NextResponse } from "next/server";

import { adminFetch } from "@/lib/flicksync";
import { hasSession, sameOrigin } from "@/lib/guard";

export const dynamic = "force-dynamic";

// Room ids are 12 Crockford base32 characters, optionally written with dashes.
const ROOM_ID = /^[0-9A-Za-z-]{12,20}$/;

/** Force-close a room (frozen or abusive). Session and same-origin required. */
export async function DELETE(_req: Request, ctx: { params: Promise<{ id: string }> }) {
  if (!(await hasSession())) {
    return NextResponse.json({ error: { code: "SIGNED_OUT" } }, { status: 401 });
  }
  if (!(await sameOrigin())) {
    return NextResponse.json({ error: { code: "FORBIDDEN" } }, { status: 403 });
  }
  const { id } = await ctx.params;
  if (!ROOM_ID.test(id)) {
    return NextResponse.json({ error: { code: "NOT_FOUND" } }, { status: 404 });
  }
  const res = await adminFetch(`rooms/${encodeURIComponent(id)}`, "DELETE");
  if (res.status === 204) return new NextResponse(null, { status: 204 });
  return NextResponse.json(res.body, { status: res.status });
}
