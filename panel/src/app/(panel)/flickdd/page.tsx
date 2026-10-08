import { ModuleGate } from "@/components/ModuleGate";
import { ButtonLink, PageHeader } from "@/components/flick/ui";

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
        actions={
          <ButtonLink href="/settings/flickdd" icon="settings">
            Settings
          </ButtonLink>
        }
      />
      <ModuleGate id="flickdd">
        <ActivePanel />
        <HistoryPanel />
        <StatsPanel />
      </ModuleGate>
    </div>
  );
}
