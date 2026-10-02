// Entry point of the panel (`npm start`, Docker CMD).
//
//   ENABLE_WEB_PANEL=false (default)  -> log a line and exit 0: nothing listens on the port.
//   ENABLE_WEB_PANEL=true             -> start the Next.js server on PORT (default 3000).
//
// Refuses to start without a real password: this panel can read the invitation (signing key)
// and close rooms.
const truthy = (v) => ["1", "true", "yes", "on"].includes(String(v ?? "").trim().toLowerCase());

if (!truthy(process.env.ENABLE_WEB_PANEL)) {
  console.log("Flick Panel is disabled (set ENABLE_WEB_PANEL=true to enable it).");
  process.exit(0);
}

const problems = [];
if ((process.env.PANEL_PASSWORD ?? "").length < 10) {
  problems.push("PANEL_PASSWORD must be set and at least 10 characters (e.g. `openssl rand -base64 18`).");
}
if ((process.env.FLICKSYNC_ADMIN_TOKEN ?? "").trim().length < 16) {
  problems.push(
    "FLICKSYNC_ADMIN_TOKEN must be set, at least 16 characters, and identical on FlickSync (e.g. `openssl rand -base64 32`).",
  );
}
if (problems.length) {
  console.error("Flick Panel cannot start:\n - " + problems.join("\n - "));
  process.exit(2);
}

process.env.PORT ||= "3000";
process.env.HOSTNAME ||= "0.0.0.0";
process.env.NODE_ENV = "production";

const { existsSync } = await import("node:fs");
const { fileURLToPath } = await import("node:url");
const standalone = fileURLToPath(new URL("./server.js", import.meta.url));
if (existsSync(standalone)) {
  // Docker image: the standalone server sits next to this file.
  await import("./server.js");
} else {
  // From a checkout after `npm run build`.
  const next = (await import("next")).default;
  const app = next({ dev: false, dir: fileURLToPath(new URL(".", import.meta.url)) });
  await app.prepare();
  const { createServer } = await import("node:http");
  const handle = app.getRequestHandler();
  createServer((req, res) => handle(req, res)).listen(Number(process.env.PORT), process.env.HOSTNAME, () => {
    console.log(`Flick Panel listening on http://${process.env.HOSTNAME}:${process.env.PORT}`);
  });
}
