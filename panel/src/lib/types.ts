// Shapes returned by FlickSync's admin API (docs/admin-api.md).

export type RoomState = "waiting" | "media_selected" | "playing" | "paused" | "empty" | "closed";
export type Presence = "connected" | "reconnecting" | "disconnected";

export interface Overview {
  version: string;
  now: number;
  uptime_secs: number;
  /** False while FlickSync is stopped; its counters are then zero. */
  running: boolean;
  accepting: boolean;
  ready: boolean;
  rooms: number;
  max_rooms: number;
  participants: number;
  connections: number;
  max_connections: number;
  rtt_avg_ms: number;
}

export interface Invite {
  url: string;
  address: string;
  tls: boolean;
  address_guessed: boolean;
  key_source: "environment" | "file";
  key_count: number;
  kid: string;
  server_id: string;
  /** Rows of "1" (dark) / "0" (light) modules. */
  qr: string[] | null;
}

export interface AdminParticipant {
  participant_id: string;
  display_name: string;
  presence: Presence;
  is_host: boolean;
  joined_at: number;
  rtt_ms: number | null;
}

export interface AdminRoom {
  room_id: string;
  share_code: string;
  state: RoomState;
  host_id: string;
  control_mode: "everyone" | "host_only";
  chat_enabled: boolean;
  max_participants: number;
  participants: AdminParticipant[];
  media_title: string | null;
  media_provider: string | null;
  media_type: string | null;
  playback: {
    state: "playing" | "paused";
    position: number;
    rate: number;
    server_time: number;
    sequence: number;
  };
  created_at: number;
  age_secs: number;
  idle_secs: number;
}

export interface RoomsResponse {
  now: number;
  rooms: AdminRoom[];
}

export interface Sample {
  t: number;
  rooms: number;
  participants: number;
  connections: number;
  rtt_ms: number;
  messages_in: number;
  corrections: number;
  seeks: number;
  reports: number;
}

export interface Stats {
  now: number;
  uptime_secs: number;
  rtt_avg_ms: number;
  totals: {
    rooms_created: number;
    rooms_destroyed: number;
    messages_in: number;
    malformed_messages: number;
    rate_limited: number;
    auth_failures: number;
    sync_reports: number;
    sync_corrections: number;
    sync_seeks: number;
  };
  drift: {
    thresholds_ms: { ignore: number; soft: number; hard: number };
    buckets: [number, number, number, number];
  };
  history_interval_secs: number;
  history: Sample[];
}

// Shapes returned by FlickDD's admin API (dd/*). Timestamps are ms since the Unix epoch.

export type DdBackend = "jellyfin" | "plex";
export type DdOutcome = "completed" | "cancelled" | "expired" | "source_changed";

export interface DdTotals {
  downloads: number;
  completed: number;
  bytes_served: number;
  resumes: number;
  upstream_errors: number;
  rejected: number;
}

export interface DdOverview {
  enabled: boolean;
  backends: { jellyfin: boolean; plex: boolean };
  limits: {
    max_parallel: number;
    max_global: number;
    rate_bps: number;
    chunk_bytes: number;
    max_range_bytes: number;
    grant_ttl_secs: number;
  };
  active: number;
  totals: DdTotals;
}

export interface DdDownload {
  download_id: string;
  /** "{server_id}/{user_id}", shown as is. */
  user_id: string;
  user_name: string;
  backend: DdBackend;
  item_id: string;
  /** May contain control characters; render as text only. */
  title: string | null;
  kind: string | null;
  size: number;
  covered: number;
  served: number;
  segments: number;
  resumes: number;
  started_at: number;
}

export interface DdActiveDownload extends DdDownload {
  last_activity: number;
  speed_bps: number;
  streaming: boolean;
}

export interface DdActive {
  now: number;
  downloads: DdActiveDownload[];
}

export interface DdFinishedDownload extends DdDownload {
  finished_at: number;
  outcome: DdOutcome;
}

export interface DdHistory {
  now: number;
  downloads: DdFinishedDownload[];
}

export interface DdStats {
  now: number;
  totals: DdTotals;
  days: { day: number; date_ms: number; bytes: number; completed: number }[];
  top_titles: {
    backend: DdBackend;
    item_id: string;
    title: string | null;
    kind: string | null;
    count: number;
    bytes: number;
  }[];
  by_backend: Record<string, { bytes: number; downloads: number; completed: number }>;
  by_outcome: Record<string, number>;
  rejected: Record<string, number>;
}

// Module lifecycle and settings (/admin/v1/modules, /admin/v1/settings).

export type ModuleId = "flicksync" | "flickdd";
export type ModuleState = "stopped" | "running" | "failed";

export interface ModuleStatus {
  id: ModuleId;
  state: ModuleState;
  /** Why the module failed to start; null otherwise. */
  message: string | null;
  /** The persisted on/off switch. */
  enabled: boolean;
  /** Running with older settings than the ones saved. */
  pending_reload: boolean;
  /** Start time (ms since the epoch) while running, else null. */
  since: number | null;
}

export interface ModulesResponse {
  modules: ModuleStatus[];
}

export type SettingsScope = "server" | "flicksync" | "flickdd";
export type FieldKind = "bool" | "int" | "float" | "text" | "choice" | "list" | "secret";
export type FieldSource = "panel" | "environment" | "default";

export interface SettingField {
  name: string;
  kind: FieldKind;
  secret: boolean;
  /** Current value; always null for a secret. */
  value: string | null;
  /** A value exists in the panel or the environment. */
  set: boolean;
  source: FieldSource;
  /** Code default; always null for a secret. */
  default: string | null;
  /** Present for `choice` fields only. */
  choices?: string[];
}

export interface SettingsView {
  scope: SettingsScope;
  revision: number;
  fields: SettingField[];
}

// Logs (/admin/v1/logs).

export type LogLevel = "error" | "warn" | "info" | "debug" | "trace";

export interface LogEntry {
  /** From 1, +1 per line within one server run. */
  seq: number;
  /** ms since the epoch. */
  ts: number;
  level: LogLevel;
  /** Module path, e.g. flicksync::api::admin. */
  target: string;
  message: string;
  /** Other fields as `name=value`, separated by spaces; "" when none. */
  fields: string;
}

export interface LogsResponse {
  /** Start of the server run (ms); a change means the server restarted. */
  boot: number;
  /** Lines the server keeps; 0 when its log buffer is off. */
  capacity: number;
  /** FLICKSYNC_LOG_LEVEL in force. */
  log_level: string;
  entries: LogEntry[];
  /** The `after` of the next request. */
  next: number;
  /** More lines are waiting: ask again at once. */
  more: boolean;
  /** Lines after the cursor already gone from the server's buffer. */
  dropped: number;
}
