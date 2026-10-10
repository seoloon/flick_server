import "server-only";

import { resolveAdminToken } from "./admin-token";

const admin = resolveAdminToken(process.env);

export const panelConfig = {
  /** Where Flick Server listens, as seen from this process. */
  flicksyncUrl: (process.env.FLICKSYNC_URL || "http://localhost:8787").replace(/\/+$/, ""),
  /** Derived from PANEL_PASSWORD. */
  adminToken: admin.token,
  adminTokenSource: admin.source,
  password: process.env.PANEL_PASSWORD ?? "",
};
