import { adminFetch } from "@/lib/flicksync";
import { hasSession } from "@/lib/guard";
import { relay, signedOut } from "@/lib/proxy";

export const dynamic = "force-dynamic";

/** State of every module. Session required. */
export async function GET() {
  if (!(await hasSession())) return signedOut();
  return relay(await adminFetch("modules"));
}
