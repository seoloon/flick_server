"use client";

import { EmptyState, Notice, Panel, Pill, Spinner } from "@/components/flick/ui";
import { BACKEND_LABEL, OUTCOME_LABEL, formatBytes, formatUtcDateTime } from "@/lib/format";
import type { DdHistory } from "@/lib/types";
import { useAdmin } from "@/lib/use-admin";

export function HistoryPanel() {
  const { data, error, errorText, loading } = useAdmin<DdHistory>("history", 10_000, "flickdd");
  const list = data?.downloads ?? [];

  return (
    <Panel title="Recent downloads">
      {loading && !data && <Spinner label="Loading history" />}
      {error && <Notice tone={data ? "warn" : "error"}>{errorText}</Notice>}
      {data && list.length === 0 && (
        <EmptyState title="Nothing finished yet">Finished downloads are listed here, newest first.</EmptyState>
      )}
      {list.length > 0 && (
        <div className="table-wrap">
          <table className="dtable">
            <thead>
              <tr>
                <th scope="col">Title</th>
                <th scope="col">User</th>
                <th scope="col">Source</th>
                <th scope="col">Served</th>
                <th scope="col">Resumes</th>
                <th scope="col">Finished</th>
                <th scope="col">Outcome</th>
              </tr>
            </thead>
            <tbody>
              {list.map((d) => (
                <tr key={`${d.download_id}-${d.finished_at}`}>
                  <td>{d.title ?? d.item_id}</td>
                  <td>{d.user_id}</td>
                  <td>{BACKEND_LABEL[d.backend] ?? d.backend}</td>
                  <td>{formatBytes(d.served)}</td>
                  <td>{d.resumes}</td>
                  <td>{formatUtcDateTime(d.finished_at)}</td>
                  <td>
                    <Pill tone={d.outcome === "completed" ? "strong" : "warn"}>
                      {OUTCOME_LABEL[d.outcome] ?? d.outcome}
                    </Pill>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </Panel>
  );
}
