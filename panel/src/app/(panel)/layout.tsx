import { redirect } from "next/navigation";

import { PanelSidebar } from "@/components/PanelSidebar";
import { hasSession } from "@/lib/guard";

export const dynamic = "force-dynamic";

export default async function PanelLayout({ children }: { children: React.ReactNode }) {
  if (!(await hasSession())) redirect("/login");
  return (
    <div className="fk-ambient panel-shell">
      <PanelSidebar />
      <main className="panel-main">{children}</main>
    </div>
  );
}
