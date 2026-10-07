import { PageHeader } from "@/components/flick/ui";

import { RoomsPanel } from "./RoomsPanel";
import { StatsPanel } from "./StatsPanel";

export const metadata = { title: "FlickSync" };

export default function FlickSyncPage() {
  return (
    <div className="panel-page">
      <PageHeader
        title="FlickSync"
        lead="Watch Together. Keep an eye on live rooms and see how well everyone stays in sync."
      />
      <RoomsPanel />
      <StatsPanel />
    </div>
  );
}
