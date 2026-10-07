import { NextResponse } from "next/server";

import { ddFetch } from "@/lib/flicksync";
import { hasSession, sameOrigin } from "@/lib/guard";

export const dynamic = "force-dynamic";

const DOWNLOAD_ID = /^[A-Za-z0-9_-]{16,32}$/;

/** Cut a download and free its slot. Session and same-origin required. */
export async function DELETE(_req: Request, ctx: { params: Promise<{ id: string }> }) {
  if (!(await hasSession())) {
    return NextResponse.json({ error: { code: "SIGNED_OUT" } }, { status: 401 });
  }
  if (!(await sameOrigin())) {
    return NextResponse.json({ error: { code: "FORBIDDEN" } }, { status: 403 });
  }
  const { id } = await ctx.params;
  if (!DOWNLOAD_ID.test(id)) {
    return NextResponse.json({ error: { code: "NOT_FOUND" } }, { status: 404 });
  }
  const res = await ddFetch(encodeURIComponent(id), "DELETE");
  if (res.status === 204) return new NextResponse(null, { status: 204 });
  return NextResponse.json(res.body, { status: res.status });
}
