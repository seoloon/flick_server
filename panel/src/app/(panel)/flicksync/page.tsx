import { PageHeader } from "@/components/flick/ui";

import { InvitePanel } from "./InvitePanel";
import { RoomsPanel } from "./RoomsPanel";
import { StatsPanel } from "./StatsPanel";

export const metadata = { title: "FlickSync" };

export default function FlickSyncPage() {
  return (
    <div className="panel-page">
      <PageHeader
        title="FlickSync"
        lead="Watch Together. Copy the invitation, keep an eye on live rooms and see how well everyone stays in sync."
      />
      <InvitePanel />
      <RoomsPanel />
      <StatsPanel />
    </div>
  );
}
