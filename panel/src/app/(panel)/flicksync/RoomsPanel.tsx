"use client";

import { useState } from "react";

import { Icon } from "@/components/flick/icons";
import { Button, Dialog, EmptyState, Notice, Panel, Pill, Spinner } from "@/components/flick/ui";
import {
  PRESENCE_LABEL,
  formatDuration,
  formatMs,
  formatPosition,
  livePosition,
  stateLabel,
} from "@/lib/format";
import type { AdminRoom, RoomsResponse } from "@/lib/types";
import { ADMIN_ERROR_TEXT, adminError, useAdmin } from "@/lib/use-admin";

/** Idle longer than this earns a caution pill: the room is up but nobody is doing anything. */
const IDLE_WARN_SECS = 10 * 60;

function RoomRow({
  room,
  now,
  open,
  onToggle,
  onClose,
}: {
  room: AdminRoom;
  now: number;
  open: boolean;
  onToggle: () => void;
  onClose: () => void;
}) {
  const connected = room.participants.filter((p) => p.presence === "connected").length;
  const host = room.participants.find((p) => p.is_host);
  const idle = room.idle_secs >= IDLE_WARN_SECS;
  const hasMedia = room.state !== "waiting" && room.state !== "empty";

  return (
    <article className="room fk-glass" data-open={open ? "" : undefined}>
      <button
        type="button"
        className="room__head fk-lift"
        aria-expanded={open}
        onClick={onToggle}
      >
        <span className="room__code">
          <strong>{room.share_code}</strong>
          <small>{room.media_title ?? (hasMedia ? "Untitled media" : "No media yet")}</small>
        </span>
        <span className="room__meta">
          <span>
            {connected} of {room.participants.length}{" "}
            {room.participants.length === 1 ? "participant" : "participants"} connected
          </span>
          <span>
            Host {host?.display_name ?? room.host_id} · Idle {formatDuration(room.idle_secs)}
          </span>
        </span>
        <span className="room__pills">
          {idle && <Pill tone="warn">No activity</Pill>}
          <Pill tone={room.state === "playing" ? "strong" : "plain"}>{stateLabel(room.state)}</Pill>
          <Icon name="chevron-right" className="room__chev" />
        </span>
      </button>

      {open && (
        <div className="room__body">
          <dl className="fk-facts">
            <dt>Room id</dt>
            <dd className="mono">{room.room_id}</dd>
            <dt>Playback</dt>
            <dd>
              {room.state === "waiting" || room.state === "empty"
                ? "Nothing selected"
                : `${room.playback.state === "playing" ? "Playing" : "Paused"} at ${formatPosition(
                    livePosition(room.playback, now),
                  )} · ${room.playback.rate}× · change ${room.playback.sequence}`}
            </dd>
            <dt>Source</dt>
            <dd>
              {room.media_provider ? `${room.media_provider}${room.media_type ? ` · ${room.media_type}` : ""}` : "None"}
            </dd>
            <dt>Settings</dt>
            <dd>
              {room.control_mode === "host_only" ? "Host controls playback" : "Everyone controls playback"} · Chat{" "}
              {room.chat_enabled ? "on" : "off"} · Up to {room.max_participants}
            </dd>
            <dt>Age</dt>
            <dd>{formatDuration(room.age_secs)}</dd>
          </dl>
          <ul className="people" aria-label="Participants">
            {room.participants.map((p) => (
              <li key={p.participant_id}>
                <span className="who">
                  {p.display_name}
                  {p.is_host ? " · Host" : ""}
                </span>
                <span className="num">{formatMs(p.rtt_ms)}</span>
                <Pill tone={p.presence === "connected" ? "plain" : "warn"}>{PRESENCE_LABEL[p.presence]}</Pill>
              </li>
            ))}
          </ul>
          <div className="row-actions">
            <Button variant="danger" size="sm" icon="trash-2" onClick={onClose}>
              Close Room
            </Button>
          </div>
        </div>
      )}
    </article>
  );
}

export function RoomsPanel() {
  const { data, error, errorText, loading, refresh } = useAdmin<RoomsResponse>("rooms", 3_000);
  const [openId, setOpenId] = useState<string | null>(null);
  const [target, setTarget] = useState<AdminRoom | null>(null);
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);

  async function closeRoom() {
    if (!target) return;
    setBusy(true);
    setFailure(null);
    try {
      const res = await fetch(`/api/flicksync/rooms/${encodeURIComponent(target.room_id)}`, { method: "DELETE" });
      // 404 means it already ended: the goal is reached either way.
      if (res.status === 204 || res.status === 404) {
        setTarget(null);
        if (openId === target.room_id) setOpenId(null);
        refresh();
      } else if (res.status === 401) {
        window.location.assign("/login");
      } else {
        setFailure(`The room could not be closed. ${adminError(await res.json().catch(() => null)).text}`);
      }
    } catch {
      setFailure(ADMIN_ERROR_TEXT.UNREACHABLE);
    } finally {
      setBusy(false);
    }
  }

  const rooms = data?.rooms ?? [];

  return (
    <Panel
      title={`Rooms${data ? ` · ${rooms.length}` : ""}`}
      actions={
        <Button variant="ghost" size="icon-sm" icon="refresh-cw" label="Refresh rooms" onClick={refresh} />
      }
    >
      {loading && !data && <Spinner label="Loading rooms" />}
      {error && <Notice tone={data ? "warn" : "error"}>{errorText}</Notice>}
      {data && rooms.length === 0 && (
        <EmptyState title="No rooms right now">
          Rooms appear here as soon as someone starts a Watch Together session.
        </EmptyState>
      )}
      {rooms.length > 0 && (
        <div className="rooms">
          {rooms.map((room) => (
            <RoomRow
              key={room.room_id}
              room={room}
              now={data?.now ?? Date.now()}
              open={openId === room.room_id}
              onToggle={() => setOpenId(openId === room.room_id ? null : room.room_id)}
              onClose={() => {
                setFailure(null);
                setTarget(room);
              }}
            />
          ))}
        </div>
      )}

      {target && (
        <Dialog
          title={`Close room ${target.share_code}?`}
          description={`Everyone in it (${target.participants.length}) is disconnected and the room is deleted. This cannot be undone. Use it for rooms that are frozen or no longer needed.`}
          onClose={() => !busy && setTarget(null)}
        >
          {failure && <Notice tone="error">{failure}</Notice>}
          <div className="row-actions">
            <Button variant="danger" icon="trash-2" onClick={closeRoom} disabled={busy}>
              {busy ? "Closing Room" : "Close Room"}
            </Button>
            <Button variant="ghost" onClick={() => setTarget(null)} disabled={busy}>
              Cancel
            </Button>
          </div>
        </Dialog>
      )}
    </Panel>
  );
}
