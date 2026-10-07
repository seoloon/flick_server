import type { IconName } from "@/components/flick/icons";

/**
 * The panel is a base for Flick Server: one entry per component. To add one, create
 * `src/app/(panel)/<id>/page.tsx` and list it here; the sidebar and Overview pick it up.
 */
export interface PanelModule {
  id: string;
  label: string;
  href: string;
  icon: IconName;
  /** One sentence for the Overview card. */
  summary: string;
}

export const MODULES: PanelModule[] = [
  {
    id: "flicksync",
    label: "FlickSync",
    href: "/flicksync",
    icon: "monitor-play",
    summary: "Watch Together: invitation link, live rooms and sync statistics.",
  },
  {
    id: "flickdd",
    label: "FlickDD",
    href: "/flickdd",
    icon: "download",
    summary: "Offline downloads: live transfers, limits and download statistics.",
  },
];

export const HOME = { id: "overview", label: "Overview", href: "/", icon: "house" as IconName };
