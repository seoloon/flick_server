import type { IconName } from "@/components/flick/icons";

import type { ModuleId } from "./types";

/**
 * The panel is a base for Flick Server: one entry per module. To add one, the server must know
 * the module id (start / stop / reload) and its settings scope; then create
 * `src/app/(panel)/<id>/page.tsx` and list it here. The sidebar, the Overview and the settings
 * pages pick it up.
 */
export interface PanelModule {
  id: ModuleId;
  label: string;
  href: string;
  /** Its settings page. */
  settingsHref: string;
  icon: IconName;
  /** One sentence for the Overview card. */
  summary: string;
}

export const MODULES: PanelModule[] = [
  {
    id: "flicksync",
    label: "FlickSync",
    href: "/flicksync",
    settingsHref: "/settings/flicksync",
    icon: "monitor-play",
    summary: "Watch Together: invitation link, live rooms and sync statistics.",
  },
  {
    id: "flickdd",
    label: "FlickDD",
    href: "/flickdd",
    settingsHref: "/settings/flickdd",
    icon: "download",
    summary: "Offline downloads: live transfers, limits and download statistics.",
  },
];

export const HOME = { id: "overview", label: "Overview", href: "/", icon: "house" as IconName };
export const SETTINGS = { id: "settings", label: "Settings", href: "/settings", icon: "settings" as IconName };
