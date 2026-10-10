import { adminFetch } from "@/lib/flicksync";
import { hasSession } from "@/lib/guard";
import { parseLogsQuery } from "@/lib/logs";
import { invalidQuery, relay, signedOut } from "@/lib/proxy";

export const dynamic = "force-dynamic";

/** The server's latest log lines after a cursor. Session required; only after, level and limit pass. */
export async function GET(req: Request) {
  if (!(await hasSession())) return signedOut();
  const parsed = parseLogsQuery(new URL(req.url).searchParams);
  if (!parsed.ok) return invalidQuery(parsed.message);
  return relay(await adminFetch(parsed.query ? `logs?${parsed.query}` : "logs"));
}
