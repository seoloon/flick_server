import "server-only";

import { NextResponse } from "next/server";

import type { UpstreamResult } from "./flicksync";

const NO_STORE = { "Cache-Control": "no-store" };

function error(status: number, code: string, message?: string) {
  return NextResponse.json({ error: message ? { code, message } : { code } }, { status, headers: NO_STORE });
}

export const signedOut = () => error(401, "SIGNED_OUT");
export const forbidden = () => error(403, "FORBIDDEN");
export const routeNotFound = () => error(404, "NOT_FOUND");
export const invalid = (message: string) => error(400, "INVALID_PAYLOAD", message);

/** Pass the server's answer on as is: its error messages explain the problem. */
export function relay(res: UpstreamResult) {
  if (res.status === 204) return new NextResponse(null, { status: 204, headers: NO_STORE });
  return NextResponse.json(res.body, { status: res.status, headers: NO_STORE });
}
