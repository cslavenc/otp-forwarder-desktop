// The only module that talks to Rust. Types mirror the serde shapes in src-tauri.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type RelayStatus =
  | { state: "connecting" }
  | { state: "connected" }
  | { state: "reconnecting"; in_secs: number; reason: string };

export interface Status {
  paired: boolean;
  relay: RelayStatus | null;
}

export interface PairingInfo {
  paired: boolean;
  relay: string | null;
  error: string | null;
}

export interface Rule {
  exe: string;
  enabled: boolean;
  press_enter: boolean;
  delay_ms: number;
}

export interface Settings {
  auto_type: boolean;
  rules: Rule[];
  clipboard_clear_secs: number;
  notifications: boolean;
  hotkey: string;
}

export type Delivery =
  | { kind: "pending" }
  | { kind: "typed"; target: string }
  | { kind: "copied"; reason: string }
  | { kind: "failed"; reason: string };

export interface RecentView {
  id: string;
  masked: string;
  sender: string;
  received_ms: number;
  expires_ms: number;
  delivery: Delivery;
}

export type Reject =
  | {
      kind:
        | "too_large"
        | "undecryptable"
        | "malformed"
        | "invalid_id"
        | "invalid_code"
        | "invalid_sender"
        | "duplicate";
    }
  | { kind: "unsupported_version"; v: number }
  | { kind: "stale"; age_s: number }
  | { kind: "from_future"; ahead_s: number };

/** Commands reject with the backend's error string. */
export const errorText = (e: unknown): string => String(e);

export const getStatus = () => invoke<Status>("get_status");
export const getPairingInfo = () => invoke<PairingInfo>("get_pairing_info");
export const pairingQr = () => invoke<string>("pairing_qr");
export const repair = () => invoke<void>("repair");
export const getSettings = () => invoke<Settings>("get_settings");
export const setSettings = (settings: Settings) => invoke<void>("set_settings", { settings });
export const getAutostart = () => invoke<boolean>("get_autostart");
export const setAutostart = (enabled: boolean) => invoke<void>("set_autostart", { enabled });
export const listRecent = () => invoke<RecentView[]>("list_recent");
export const revealCode = (id: string) => invoke<string>("reveal_code", { id });
export const copyCode = (id: string) => invoke<void>("copy_code", { id });
export const sendTestCode = () => invoke<void>("send_test_code");

const on = <T>(event: string, handler: (payload: T) => void) =>
  listen<T>(event, (e) => handler(e.payload));

export const onRelayStatus = (h: (s: RelayStatus) => void) => on("relay-status", h);
export const onRecent = (h: (r: RecentView[]) => void) => on("recent", h);
export const onRejected = (h: (r: Reject) => void) => on("rejected", h);
export const onHidden = (h: () => void) => on<null>("hidden", () => h());

/**
 * Waits for `listeners` to be registered, then runs `ready` (the initial fetch), so no event is
 * lost in between. Listener and `ready` errors go to `onError`. Returns a sync cleanup for `$effect`.
 */
export function subscribe(
  listeners: Promise<UnlistenFn>[],
  ready: () => Promise<unknown>,
  onError: (message: string) => void,
): () => void {
  let closed = false;
  const settled = Promise.allSettled(listeners);
  settled
    .then(async (results) => {
      for (const r of results) if (r.status === "rejected") onError(errorText(r.reason));
      if (!closed) await ready();
    })
    .catch((e) => onError(errorText(e)));
  return () => {
    closed = true;
    settled
      .then((results) => results.forEach((r) => r.status === "fulfilled" && r.value()))
      .catch((e) => onError(errorText(e)));
  };
}
