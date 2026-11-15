import {
  ButtonItem,
  PanelSection,
  PanelSectionRow,
  ToggleField,
  staticClasses,
} from "@decky/ui";
import { callable, definePlugin } from "@decky/api";
import { useEffect, useState, FC } from "react";

interface CrdStatus {
  registered: boolean;
  host_name: string;
  enabled: boolean;
  host_active: boolean;
  bridge_active: boolean;
  client_connected: boolean;
}

const getStatus = callable<[], CrdStatus>("get_status");
const setEnabled = callable<[boolean], CrdStatus>("set_enabled");
const restartService = callable<[], CrdStatus>("restart_service");

const Content: FC = () => {
  const [status, setStatus] = useState<CrdStatus | null>(null);
  const [busy, setBusy] = useState<boolean>(false);

  const refresh = async () => {
    try {
      const s = await getStatus();
      setStatus(s);
    } catch (e) {
      console.error("Failed to fetch CRD status", e);
    }
  };

  useEffect(() => {
    refresh();
    const timer = setInterval(refresh, 3000);
    return () => clearInterval(timer);
  }, []);

  const onToggle = async (checked: boolean) => {
    setBusy(true);
    try {
      const s = await setEnabled(checked);
      setStatus(s);
    } finally {
      setBusy(false);
    }
  };

  const onRestart = async () => {
    setBusy(true);
    try {
      const s = await restartService();
      setStatus(s);
    } finally {
      setBusy(false);
    }
  };

  return (
    <PanelSection title="Chrome Remote Desktop">
      <PanelSectionRow>
        <ToggleField
          label="Enable Remote Access"
          description={
            status?.registered
              ? `Host: ${status.host_name}`
              : "Not registered yet — run 'steamos-crd setup'"
          }
          checked={status?.enabled ?? false}
          disabled={busy || !status?.registered}
          onChange={onToggle}
        />
      </PanelSectionRow>

      <PanelSectionRow>
        <div style={{ fontSize: "12px", opacity: 0.85, padding: "4px 0" }}>
          <div>
            <strong>Session State:</strong>{" "}
            {status?.client_connected
              ? "Active Remote Client Connected"
              : status?.enabled
              ? "Ready (Idle — 0% GPU/CPU)"
              : "Stopped"}
          </div>
          <div>
            <strong>Bridge Daemon:</strong>{" "}
            {status?.bridge_active ? "Running" : "Stopped"}
          </div>
        </div>
      </PanelSectionRow>

      <PanelSectionRow>
        <ButtonItem layout="below" disabled={busy} onClick={onRestart}>
          Restart CRD Stack
        </ButtonItem>
      </PanelSectionRow>
    </PanelSection>
  );
};

export default definePlugin(() => ({
  name: "SteamOS Chromoting",
  titleView: <div className={staticClasses.Title}>SteamOS Chromoting</div>,
  content: <Content />,
  icon: <div style={{ fontWeight: 700 }}>CRD</div>,
}));
