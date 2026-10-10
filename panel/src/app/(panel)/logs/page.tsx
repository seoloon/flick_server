import { ButtonLink, PageHeader } from "@/components/flick/ui";

import { LogsView } from "./LogsView";

export const metadata = { title: "Logs" };

export default function LogsPage() {
  return (
    <div className="panel-page">
      <PageHeader
        title="Logs"
        lead="What Flick Server is doing, live. The server keeps its latest lines in memory only: they are gone after a restart."
        actions={
          <ButtonLink href="/settings/server" icon="settings">
            Server Settings
          </ButtonLink>
        }
      />
      <LogsView />
    </div>
  );
}
