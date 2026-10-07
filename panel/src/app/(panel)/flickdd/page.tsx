import { PageHeader } from "@/components/flick/ui";

import { DdGate } from "./DdGate";
import { ActivePanel } from "./ActivePanel";
import { HistoryPanel } from "./HistoryPanel";
import { StatsPanel } from "./StatsPanel";

export const metadata = { title: "FlickDD" };

export default function FlickDDPage() {
  return (
    <div className="panel-page">
      <PageHeader
        title="FlickDD"
        lead="Offline downloads. See live transfers, cut one if needed and follow how much is downloaded."
      />
      <DdGate>
        <ActivePanel />
        <HistoryPanel />
        <StatsPanel />
      </DdGate>
    </div>
  );
}
