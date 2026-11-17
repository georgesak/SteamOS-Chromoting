export interface CrdSettings {
  autostart: boolean;
  resolution: string;
  max_fps: number;
  audio_enabled: boolean;
  clipboard_sync: boolean;
  gaming_chords: boolean;
  composite_force: boolean;
  always_active: boolean;
  display: string;
  log_level: "error" | "warn" | "info" | "debug";
}

export type InstallState =
  | "idle"
  | "installing"
  | "updating"
  | "uninstalling"
  | "error";

export interface CrdStatus {
  installed: boolean;
  crd_deb_installed: boolean;
  bridge_installed: boolean;
  server_version: string | null;
  registered: boolean;
  host_name: string;
  host_owner: string | null;
  host_id: string | null;
  default_hostname: string;
  enabled: boolean;
  host_active: boolean;
  bridge_active: boolean;
  xvfb_active: boolean;
  audio_active: boolean;
  client_connected: boolean;
  uinput_writable: boolean;
  install_state: InstallState;
  install_step: string;
  install_percent: number;
  install_error: string | null;
  install_log: string[];
  settings: CrdSettings;
}

export interface InstallProgressEvent {
  state: InstallState;
  step: string;
  percent: number;
  error: string | null;
  log: string[];
}

export interface ActionResult {
  ok: boolean;
  error?: string | null;
  status: CrdStatus;
}
