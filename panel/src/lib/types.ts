// Shapes returned by FlickSync's admin API (docs/admin-api.md).

export type RoomState = "waiting" | "media_selected" | "playing" | "paused" | "empty" | "closed";
export type Presence = "connected" | "reconnecting" | "disconnected";

export interface Overview {
  version: string;
  now: number;
  uptime_secs: number;
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
