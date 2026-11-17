import { callable, toaster } from "@decky/api";
import type { ActionResult, CrdSettings, CrdStatus } from "./types";

export const getStatus = callable<[], CrdStatus>("get_status");
export const setEnabled = callable<[boolean], CrdStatus>("set_enabled");
export const restartService = callable<[], CrdStatus>("restart_service");

export const getSettings = callable<[], CrdSettings>("get_settings");
export const saveSettings = callable<[Partial<CrdSettings>], CrdStatus>(
  "save_settings"
);
export const resetSettings = callable<[], CrdStatus>("reset_settings");

export const installServer = callable<[], CrdStatus>("install_server");
export const updateServer = callable<[], CrdStatus>("update_server");
export const uninstallServer = callable<[boolean], CrdStatus>(
  "uninstall_server"
);

export const setupHost = callable<[string, string, string], ActionResult>(
  "setup_host"
);
export const changePin = callable<[string], ActionResult>("change_pin");
export const changeHostName = callable<[string], ActionResult>(
  "change_host_name"
);

export const getClipboardText = callable<[], { text: string }>(
  "get_clipboard_text"
);
export const getLogs = callable<[string, number], { logs: string }>("get_logs");

export function notify(title: string, body: string): void {
  try {
    toaster.toast({ title, body });
  } catch (e) {
    console.warn("Toast failed:", e);
  }
}
