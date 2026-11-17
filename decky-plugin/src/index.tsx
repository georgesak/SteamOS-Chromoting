import {
  ButtonItem,
  DropdownItem,
  PanelSection,
  PanelSectionRow,
  ProgressBarWithInfo,
  ToggleField,
  showModal,
  staticClasses,
} from "@decky/ui";
import {
  addEventListener,
  definePlugin,
  removeEventListener,
} from "@decky/api";
import { FC, useCallback, useEffect, useState } from "react";
import {
  getStatus,
  installServer,
  notify,
  resetSettings,
  restartService,
  saveSettings,
  setEnabled,
  updateServer,
} from "./api";
import {
  CRD_ACCESS_URL,
  ChangePinModal,
  HEADLESS_AUTH_URL,
  LogsModal,
  RenameHostModal,
  SetupHostModal,
  UninstallModal,
  openExternalUrlCleanly,
} from "./modals";
import type { CrdSettings, CrdStatus, InstallProgressEvent } from "./types";

const RESOLUTION_OPTIONS = [
  { label: "1280×800 (Deck Native 16:10)", data: "1280x800x24" },
  { label: "1920×1080 (1080p Full HD)", data: "1920x1080x24" },
  { label: "1920×1200 (1200p WUXGA)", data: "1920x1200x24" },
  { label: "2560×1440 (1440p QHD)", data: "2560x1440x24" },
];

const FPS_OPTIONS = [
  { label: "60 FPS (Smooth)", data: 60 },
  { label: "45 FPS (Balanced)", data: 45 },
  { label: "30 FPS (Battery Saver)", data: 30 },
  { label: "15 FPS (Low Bandwidth)", data: 15 },
];

const LOG_LEVEL_OPTIONS = [
  { label: "Info (Default)", data: "info" },
  { label: "Debug (Verbose)", data: "debug" },
  { label: "Warnings Only", data: "warn" },
  { label: "Errors Only", data: "error" },
];

const ChromotingIcon: FC = () => (
  <svg
    width="1em"
    height="1em"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    strokeWidth="2"
    strokeLinecap="round"
    strokeLinejoin="round"
  >
    <rect x="2" y="3" width="20" height="14" rx="2" ry="2" />
    <line x1="8" y1="21" x2="16" y2="21" />
    <line x1="12" y1="17" x2="12" y2="21" />
    <circle cx="12" cy="10" r="3" />
  </svg>
);

const StatusBadge: FC<{ status: CrdStatus }> = ({ status }) => {
  let dotColor = "#8b929a";
  let stateText = "Stopped";

  if (!status.registered) {
    dotColor = "#f59e0b";
    stateText = "Setup Required (Unregistered)";
  } else if (status.client_connected) {
    dotColor = "#22c55e";
    stateText = "Remote Client Connected";
  } else if (status.enabled) {
    dotColor = "#38bdf8";
    stateText = "Ready (Idle — 0% GPU/CPU)";
  }

  return (
    <div
      style={{
        padding: "8px 10px",
        borderRadius: "6px",
        background: "rgba(255, 255, 255, 0.06)",
        fontSize: "12px",
        lineHeight: 1.45,
      }}
    >
      <div style={{ display: "flex", alignItems: "center", gap: "7px", fontWeight: 600 }}>
        <span
          style={{
            display: "inline-block",
            width: "8px",
            height: "8px",
            borderRadius: "50%",
            backgroundColor: dotColor,
            boxShadow: `0 0 6px ${dotColor}`,
          }}
        />
        <span>{stateText}</span>
      </div>

      {status.registered && (
        <div style={{ opacity: 0.85, marginTop: "4px" }}>
          <strong>Host:</strong> {status.host_name}
          {status.host_owner ? ` (${status.host_owner})` : ""}
        </div>
      )}

      <div style={{ opacity: 0.75, marginTop: "2px" }}>
        <strong>Services:</strong>{" "}
        Host {status.host_active ? "On" : "Off"} • Bridge{" "}
        {status.bridge_active ? "On" : "Off"} • Xvfb{" "}
        {status.xvfb_active ? "On" : "Off"}
      </div>

      {!status.uinput_writable && (
        <div style={{ color: "#fbbf24", marginTop: "4px" }}>
          Warning: /dev/uinput is not currently writable by user.
        </div>
      )}
    </div>
  );
};

const Content: FC = () => {
  const [status, setStatus] = useState<CrdStatus | null>(null);
  const [busy, setBusy] = useState<boolean>(false);

  const refresh = useCallback(async () => {
    try {
      const s = await getStatus();
      setStatus(s);
    } catch (e) {
      console.error("Failed to fetch SteamOS Chromoting status:", e);
    }
  }, []);

  useEffect(() => {
    refresh();
    const timer = setInterval(refresh, 3000);

    const onProgress = (ev: InstallProgressEvent) => {
      setStatus((prev) => {
        if (!prev) {
          return prev;
        }
        return {
          ...prev,
          install_state: ev.state,
          install_step: ev.step,
          install_percent: ev.percent,
          install_error: ev.error,
          install_log: ev.log,
        };
      });
      if (ev.state === "idle" && ev.percent === 100) {
        refresh();
      }
    };

    const registeredListener = addEventListener<
      [InstallProgressEvent]
    >("crd_install_progress", onProgress);

    return () => {
      clearInterval(timer);
      removeEventListener("crd_install_progress", registeredListener);
    };
  }, [refresh]);

  const isJobRunning =
    status?.install_state === "installing" ||
    status?.install_state === "updating" ||
    status?.install_state === "uninstalling";

  const handleInstall = async () => {
    setBusy(true);
    try {
      const s = await installServer();
      setStatus(s);
    } catch (e) {
      notify("SteamOS Chromoting", `Failed to start installer: ${String(e)}`);
    } finally {
      setBusy(false);
    }
  };

  const handleUpdate = async () => {
    setBusy(true);
    try {
      const s = await updateServer();
      setStatus(s);
      notify("SteamOS Chromoting", "Updating server in background...");
    } catch (e) {
      notify("SteamOS Chromoting", `Update failed to start: ${String(e)}`);
    } finally {
      setBusy(false);
    }
  };

  const handleToggleEnabled = async (checked: boolean) => {
    setBusy(true);
    try {
      const s = await setEnabled(checked);
      setStatus(s);
    } catch (e) {
      notify("SteamOS Chromoting", `Failed toggling service: ${String(e)}`);
    } finally {
      setBusy(false);
    }
  };

  const handleRestart = async () => {
    setBusy(true);
    try {
      const s = await restartService();
      setStatus(s);
      notify("SteamOS Chromoting", "Restarted CRD services.");
    } catch (e) {
      notify("SteamOS Chromoting", `Restart failed: ${String(e)}`);
    } finally {
      setBusy(false);
    }
  };

  const handleSettingUpdate = async (patch: Partial<CrdSettings>) => {
    if (!status) {
      return;
    }
    // Optimistically reflect change in UI while saving
    setStatus({
      ...status,
      settings: {
        ...status.settings,
        ...patch,
      },
    });
    setBusy(true);
    try {
      const updatedStatus = await saveSettings(patch);
      setStatus(updatedStatus);
    } catch (e) {
      notify("SteamOS Chromoting", `Failed saving setting: ${String(e)}`);
      await refresh();
    } finally {
      setBusy(false);
    }
  };

  const handleResetSettings = async () => {
    setBusy(true);
    try {
      const updatedStatus = await resetSettings();
      setStatus(updatedStatus);
      notify("SteamOS Chromoting", "Restored default settings.");
    } catch (e) {
      notify("SteamOS Chromoting", `Reset failed: ${String(e)}`);
    } finally {
      setBusy(false);
    }
  };

  const openSetupModal = () => {
    showModal(
      <SetupHostModal
        initialHostName={
          status?.registered
            ? status.host_name
            : status?.default_hostname || "steamdeck"
        }
        onSuccess={(s) => setStatus(s)}
      />
    );
  };

  if (!status) {
    return (
      <PanelSection title="Chrome Remote Desktop">
        <PanelSectionRow>
          <div style={{ fontSize: "13px", opacity: 0.8 }}>
            Checking SteamOS Chromoting status...
          </div>
        </PanelSectionRow>
      </PanelSection>
    );
  }

  // =========================================================================
  // View 1: Server Not Installed Yet
  // =========================================================================
  if (!status.installed) {
    const lastLogLine =
      status.install_log.length > 0
        ? status.install_log[status.install_log.length - 1]
        : "";

    return (
      <PanelSection title="Server Setup">
        <PanelSectionRow>
          <div
            style={{
              padding: "8px 10px",
              borderRadius: "6px",
              background: "rgba(255, 255, 255, 0.06)",
              fontSize: "12px",
              lineHeight: 1.45,
            }}
          >
            <div style={{ fontWeight: 600, marginBottom: "4px" }}>
              Server Not Installed
            </div>
            <div style={{ opacity: 0.85 }}>
              Installs the rootless Chrome Remote Desktop host, PipeWire/uinput
              bridge, and user services in{" "}
              <code>~/.local/share/steamos-chromoting</code> (survives SteamOS
              updates).
            </div>
          </div>
        </PanelSectionRow>

        {isJobRunning && (
          <PanelSectionRow>
            <ProgressBarWithInfo
              label={status.install_step || "Installing..."}
              description={lastLogLine}
              nProgress={status.install_percent}
            />
          </PanelSectionRow>
        )}

        {status.install_error && !isJobRunning && (
          <PanelSectionRow>
            <div
              style={{
                padding: "8px 10px",
                borderRadius: "6px",
                background: "rgba(220, 53, 69, 0.2)",
                border: "1px solid rgba(220, 53, 69, 0.5)",
                color: "#ff9aa2",
                fontSize: "12px",
                whiteSpace: "pre-wrap",
              }}
            >
              {status.install_error}
            </div>
          </PanelSectionRow>
        )}

        <PanelSectionRow>
          <ButtonItem
            layout="below"
            disabled={busy || isJobRunning}
            onClick={handleInstall}
          >
            {isJobRunning
              ? "Installing Server..."
              : status.install_error
              ? "Retry Server Install"
              : "Install Server"}
          </ButtonItem>
        </PanelSectionRow>

        {status.install_log.length > 0 && !isJobRunning && (
          <PanelSectionRow>
            <ButtonItem
              layout="below"
              onClick={() =>
                showModal(
                  <LogsModal
                    title="Installer Output"
                    initialLogs={status.install_log.join("\n")}
                  />
                )
              }
            >
              View Installer Log
            </ButtonItem>
          </PanelSectionRow>
        )}
      </PanelSection>
    );
  }

  // =========================================================================
  // View 2: Server Installed — Controls, Settings & Management
  // =========================================================================
  const { settings } = status;
  const resolutionOptions = RESOLUTION_OPTIONS.some(
    (o) => o.data === settings.resolution
  )
    ? RESOLUTION_OPTIONS
    : [
        { label: `${settings.resolution} (Custom)`, data: settings.resolution },
        ...RESOLUTION_OPTIONS,
      ];

  const fpsOptions = FPS_OPTIONS.some((o) => o.data === settings.max_fps)
    ? FPS_OPTIONS
    : [{ label: `${settings.max_fps} FPS`, data: settings.max_fps }, ...FPS_OPTIONS];

  return (
    <>
      <PanelSection title="Remote Access">
        <PanelSectionRow>
          <StatusBadge status={status} />
        </PanelSectionRow>

        {isJobRunning && (
          <PanelSectionRow>
            <ProgressBarWithInfo
              label={status.install_step || "Working..."}
              nProgress={status.install_percent}
            />
          </PanelSectionRow>
        )}

        <PanelSectionRow>
          <ToggleField
            label="Enable Remote Access"
            description={
              status.registered
                ? "Run Chrome Remote Desktop host & capture bridge"
                : "Pair your Google account below first"
            }
            checked={status.enabled}
            disabled={busy || isJobRunning || !status.registered}
            onChange={handleToggleEnabled}
          />
        </PanelSectionRow>

        {!status.registered ? (
          <>
            <PanelSectionRow>
              <ButtonItem
                layout="below"
                disabled={busy || isJobRunning}
                onClick={() => openExternalUrlCleanly(HEADLESS_AUTH_URL)}
              >
                1. Open Google Auth Page
              </ButtonItem>
            </PanelSectionRow>
            <PanelSectionRow>
              <ButtonItem
                layout="below"
                disabled={busy || isJobRunning}
                onClick={openSetupModal}
              >
                2. Enter Auth Code & Set PIN
              </ButtonItem>
            </PanelSectionRow>
          </>
        ) : (
          <PanelSectionRow>
            <ButtonItem
              layout="below"
              disabled={busy || isJobRunning}
              onClick={handleRestart}
            >
              Restart Services
            </ButtonItem>
          </PanelSectionRow>
        )}
      </PanelSection>

      <PanelSection title="Settings">
        <PanelSectionRow>
          <ToggleField
            label="Start on Boot"
            description="Automatically start remote access when SteamOS starts"
            checked={settings.autostart}
            disabled={busy || isJobRunning}
            onChange={(val) => handleSettingUpdate({ autostart: val })}
          />
        </PanelSectionRow>

        <PanelSectionRow>
          <DropdownItem
            label="Display Resolution"
            description="Virtual Xvfb screen resolution"
            selectedOption={settings.resolution}
            rgOptions={resolutionOptions}
            disabled={busy || isJobRunning}
            onChange={(opt) =>
              handleSettingUpdate({ resolution: String(opt.data) })
            }
          />
        </PanelSectionRow>

        <PanelSectionRow>
          <DropdownItem
            label="Max Frame Rate"
            description="Target capture & stream frame rate"
            selectedOption={settings.max_fps}
            rgOptions={fpsOptions}
            disabled={busy || isJobRunning}
            onChange={(opt) =>
              handleSettingUpdate({ max_fps: Number(opt.data) })
            }
          />
        </PanelSectionRow>

        <PanelSectionRow>
          <ToggleField
            label="Stream Remote Audio"
            description="Stream system audio without muting Deck speakers"
            checked={settings.audio_enabled}
            disabled={busy || isJobRunning}
            onChange={(val) => handleSettingUpdate({ audio_enabled: val })}
          />
        </PanelSectionRow>

        <PanelSectionRow>
          <ToggleField
            label="Clipboard Sync"
            description="Bidirectional UTF-8 copy & paste across sessions"
            checked={settings.clipboard_sync}
            disabled={busy || isJobRunning}
            onChange={(val) => handleSettingUpdate({ clipboard_sync: val })}
          />
        </PanelSectionRow>

        <PanelSectionRow>
          <ToggleField
            label="Gaming Mode Shortcuts"
            description="Ctrl+Alt+S for STEAM menu, Ctrl+Alt+Q for QAM (...)"
            checked={settings.gaming_chords}
            disabled={busy || isJobRunning}
            onChange={(val) => handleSettingUpdate({ gaming_chords: val })}
          />
        </PanelSectionRow>

        <PanelSectionRow>
          <ToggleField
            label="Capture Steam Overlays"
            description="Composite STEAM & QAM overlays into remote stream"
            checked={settings.composite_force}
            disabled={busy || isJobRunning}
            onChange={(val) => handleSettingUpdate({ composite_force: val })}
          />
        </PanelSectionRow>

        <PanelSectionRow>
          <ToggleField
            label="Keep Capture Warm When Idle"
            description="Disable 0% CPU/GPU idle pause when disconnected"
            checked={settings.always_active}
            disabled={busy || isJobRunning}
            onChange={(val) => handleSettingUpdate({ always_active: val })}
          />
        </PanelSectionRow>

        <PanelSectionRow>
          <DropdownItem
            label="Bridge Log Level"
            description="Diagnostic logging verbosity"
            selectedOption={settings.log_level}
            rgOptions={LOG_LEVEL_OPTIONS}
            disabled={busy || isJobRunning}
            onChange={(opt) =>
              handleSettingUpdate({
                log_level: opt.data as CrdSettings["log_level"],
              })
            }
          />
        </PanelSectionRow>

        <PanelSectionRow>
          <ButtonItem
            layout="below"
            disabled={busy || isJobRunning}
            onClick={handleResetSettings}
          >
            Reset Settings to Defaults
          </ButtonItem>
        </PanelSectionRow>
      </PanelSection>

      <PanelSection title="Host & Server Management">
        {status.registered && (
          <>
            <PanelSectionRow>
              <ButtonItem
                layout="below"
                disabled={busy || isJobRunning}
                onClick={() =>
                  showModal(
                    <ChangePinModal onSuccess={(s) => setStatus(s)} />
                  )
                }
              >
                Change PIN
              </ButtonItem>
            </PanelSectionRow>

            <PanelSectionRow>
              <ButtonItem
                layout="below"
                disabled={busy || isJobRunning}
                onClick={() =>
                  showModal(
                    <RenameHostModal
                      currentName={status.host_name}
                      onSuccess={(s) => setStatus(s)}
                    />
                  )
                }
              >
                Change Device Name
              </ButtonItem>
            </PanelSectionRow>
          </>
        )}

        <PanelSectionRow>
          <ButtonItem
            layout="below"
            disabled={busy || isJobRunning}
            onClick={openSetupModal}
          >
            {status.registered
              ? "Re-pair Google Account"
              : "Pair Google Account & PIN"}
          </ButtonItem>
        </PanelSectionRow>

        <PanelSectionRow>
          <ButtonItem
            layout="below"
            disabled={busy || isJobRunning}
            onClick={() => openExternalUrlCleanly(CRD_ACCESS_URL)}
          >
            Open CRD Web Portal
          </ButtonItem>
        </PanelSectionRow>

        <PanelSectionRow>
          <ButtonItem
            layout="below"
            disabled={isJobRunning}
            onClick={() => showModal(<LogsModal />)}
          >
            View Service Logs
          </ButtonItem>
        </PanelSectionRow>

        <PanelSectionRow>
          <ButtonItem
            layout="below"
            description={
              status.server_version
                ? `Installed version: v${status.server_version}`
                : "Download & install latest GitHub release"
            }
            disabled={busy || isJobRunning}
            onClick={handleUpdate}
          >
            Update Server
          </ButtonItem>
        </PanelSectionRow>

        <PanelSectionRow>
          <ButtonItem
            layout="below"
            disabled={busy || isJobRunning}
            onClick={() =>
              showModal(<UninstallModal onSuccess={(s) => setStatus(s)} />)
            }
          >
            Uninstall Server
          </ButtonItem>
        </PanelSectionRow>
      </PanelSection>
    </>
  );
};

export default definePlugin(() => ({
  name: "SteamOS Chromoting",
  titleView: <div className={staticClasses.Title}>SteamOS Chromoting</div>,
  content: <Content />,
  icon: <ChromotingIcon />,
}));
