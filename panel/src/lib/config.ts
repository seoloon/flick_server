import "server-only";

export const panelConfig = {
  /** Where FlickSync listens, as seen from this process. */
  flicksyncUrl: (process.env.FLICKSYNC_URL || "http://localhost:8787").replace(/\/+$/, ""),
  adminToken: process.env.FLICKSYNC_ADMIN_TOKEN?.trim() || "",
  password: process.env.PANEL_PASSWORD ?? "",
};
