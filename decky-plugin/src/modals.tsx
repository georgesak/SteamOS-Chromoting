import {
  ConfirmModal,
  DialogBody,
  DialogButton,
  DialogControlsSection,
  DialogHeader,
  DropdownItem,
  Focusable,
  ModalRoot,
  Navigation,
  TextField,
  ToggleField,
} from "@decky/ui";
import { FC, useEffect, useState } from "react";
import {
  changeHostName,
  changePin,
  getClipboardText,
  getLogs,
  notify,
  setupHost,
  uninstallServer,
} from "./api";
import type { CrdStatus } from "./types";

export const HEADLESS_AUTH_URL = "https://remotedesktop.google.com/headless";
export const CRD_ACCESS_URL = "https://remotedesktop.google.com/access";

let draftAuthCode = "";
let draftHostName = "";

export function openExternalUrlCleanly(
  url: string,
  closeModal?: () => void
): void {
  if (closeModal) {
    closeModal();
  }
  Navigation.CloseSideMenus();
  setTimeout(() => {
    Navigation.NavigateToExternalWeb(url);
  }, 100);
}

interface SetupHostModalProps {
  initialHostName: string;
  onSuccess: (status: CrdStatus) => void;
  closeModal?: () => void;
}

export const SetupHostModal: FC<SetupHostModalProps> = ({
  initialHostName,
  onSuccess,
  closeModal,
}) => {
  const [authInput, setAuthInput] = useState<string>(draftAuthCode);
  const [hostName, setHostName] = useState<string>(
    draftHostName || initialHostName || "steamdeck"
  );
  const [pin, setPin] = useState<string>("");
  const [confirmPin, setConfirmPin] = useState<string>("");
  const [submitting, setSubmitting] = useState<boolean>(false);
  const [error, setError] = useState<string | null>(null);

  const handleAuthChange = (val: string) => {
    draftAuthCode = val;
    setAuthInput(val);
  };

  const handleNameChange = (val: string) => {
    draftHostName = val;
    setHostName(val);
  };

  const handlePasteClipboard = async () => {
    setError(null);
    try {
      if (navigator.clipboard && navigator.clipboard.readText) {
        const clip = await navigator.clipboard.readText();
        if (clip && clip.trim()) {
          handleAuthChange(clip.trim());
          notify("SteamOS Chromoting", "Pasted authorization text from clipboard.");
          return;
        }
      }
    } catch {
      // Fall back to backend clipboard probe
    }

    try {
      const res = await getClipboardText();
      if (res.text && res.text.trim()) {
        handleAuthChange(res.text.trim());
        notify("SteamOS Chromoting", "Pasted authorization text from clipboard.");
      } else {
        setError(
          "Clipboard looks empty. Open Google Auth Page, authorize, click Copy on 'Debian Linux', then try again."
        );
      }
    } catch (e) {
      setError(`Could not read clipboard: ${String(e)}`);
    }
  };

  const handleSubmit = async () => {
    setError(null);
    if (!authInput.trim()) {
      setError(
        "Paste the 'Debian Linux' command (or --code) from Google's setup page."
      );
      return;
    }
    if (!/^\d{6,}$/.test(pin.trim())) {
      setError("PIN must be at least 6 numeric digits (0–9).");
      return;
    }
    if (pin.trim() !== confirmPin.trim()) {
      setError("PIN and Confirm PIN do not match.");
      return;
    }

    setSubmitting(true);
    try {
      const res = await setupHost(authInput.trim(), pin.trim(), hostName.trim());
      if (!res.ok) {
        setError(res.error || "Host registration failed.");
        return;
      }
      draftAuthCode = "";
      onSuccess(res.status);
      notify(
        "SteamOS Chromoting",
        `Registered '${res.status.host_name}' and started remote access!`
      );
      closeModal?.();
    } catch (e) {
      setError(`Registration failed: ${String(e)}`);
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <ModalRoot onCancel={closeModal} closeModal={closeModal}>
      <DialogHeader>Pair Google Account & Set PIN</DialogHeader>
      <DialogBody>
        <DialogControlsSection>
          <div style={{ fontSize: "13px", opacity: 0.9, marginBottom: "10px", lineHeight: 1.4 }}>
            1. Open Google&apos;s Headless Setup page, sign in, click{" "}
            <strong>Begin → Next → Authorize</strong>, and copy the{" "}
            <strong>Debian Linux</strong> command.
          </div>

          <Focusable
            style={{ display: "flex", gap: "8px", marginBottom: "12px" }}
          >
            <DialogButton
              style={{ flex: 1 }}
              disabled={submitting}
              onClick={() => openExternalUrlCleanly(HEADLESS_AUTH_URL, closeModal)}
            >
              1. Open Google Auth Page
            </DialogButton>
            <DialogButton
              style={{ flex: 1 }}
              disabled={submitting}
              onClick={handlePasteClipboard}
            >
              Paste from Clipboard
            </DialogButton>
          </Focusable>

          <TextField
            label="2. Debian Linux Command or --code"
            value={authInput}
            disabled={submitting}
            bShowClearAction={true}
            onChange={(e) => handleAuthChange(e.target.value)}
          />

          <TextField
            label="3. Device Display Name"
            value={hostName}
            disabled={submitting}
            onChange={(e) => handleNameChange(e.target.value)}
          />

          <TextField
            label="4. Remote Access PIN (6+ digits)"
            value={pin}
            mustBeNumeric={true}
            bIsPassword={true}
            disabled={submitting}
            onChange={(e) => setPin(e.target.value)}
          />

          <TextField
            label="Confirm PIN"
            value={confirmPin}
            mustBeNumeric={true}
            bIsPassword={true}
            disabled={submitting}
            onChange={(e) => setConfirmPin(e.target.value)}
          />

          {error && (
            <div
              style={{
                marginTop: "10px",
                padding: "8px 10px",
                borderRadius: "6px",
                background: "rgba(220, 53, 69, 0.2)",
                border: "1px solid rgba(220, 53, 69, 0.55)",
                color: "#ff9aa2",
                fontSize: "12px",
                whiteSpace: "pre-wrap",
              }}
            >
              {error}
            </div>
          )}

          <Focusable
            style={{ display: "flex", gap: "8px", marginTop: "14px" }}
          >
            <DialogButton
              style={{ flex: 1 }}
              disabled={submitting}
              onClick={handleSubmit}
            >
              {submitting ? "Registering Host..." : "Register & Start Host"}
            </DialogButton>
            <DialogButton
              style={{ flex: 0.6 }}
              disabled={submitting}
              onClick={() => closeModal?.()}
            >
              Cancel
            </DialogButton>
          </Focusable>
        </DialogControlsSection>
      </DialogBody>
    </ModalRoot>
  );
};

interface ChangePinModalProps {
  onSuccess: (status: CrdStatus) => void;
  closeModal?: () => void;
}

export const ChangePinModal: FC<ChangePinModalProps> = ({
  onSuccess,
  closeModal,
}) => {
  const [pin, setPin] = useState<string>("");
  const [confirmPin, setConfirmPin] = useState<string>("");
  const [saving, setSaving] = useState<boolean>(false);
  const [error, setError] = useState<string | null>(null);

  const handleSave = async () => {
    setError(null);
    if (!/^\d{6,}$/.test(pin.trim())) {
      setError("PIN must be at least 6 numeric digits (0–9).");
      return;
    }
    if (pin.trim() !== confirmPin.trim()) {
      setError("PINs do not match.");
      return;
    }

    setSaving(true);
    try {
      const res = await changePin(pin.trim());
      if (!res.ok) {
        setError(res.error || "Failed to update PIN.");
        return;
      }
      onSuccess(res.status);
      notify("SteamOS Chromoting", "Remote access PIN updated!");
      closeModal?.();
    } catch (e) {
      setError(`Failed to update PIN: ${String(e)}`);
    } finally {
      setSaving(false);
    }
  };

  return (
    <ModalRoot onCancel={closeModal} closeModal={closeModal}>
      <DialogHeader>Change Remote Access PIN</DialogHeader>
      <DialogBody>
        <DialogControlsSection>
          <div style={{ fontSize: "13px", opacity: 0.85, marginBottom: "10px" }}>
            Choose a new numeric PIN (6 or more digits). Your Google account
            registration stays linked.
          </div>

          <TextField
            label="New PIN (6+ digits)"
            value={pin}
            mustBeNumeric={true}
            bIsPassword={true}
            disabled={saving}
            onChange={(e) => setPin(e.target.value)}
          />

          <TextField
            label="Confirm New PIN"
            value={confirmPin}
            mustBeNumeric={true}
            bIsPassword={true}
            disabled={saving}
            onChange={(e) => setConfirmPin(e.target.value)}
          />

          {error && (
            <div
              style={{
                marginTop: "10px",
                padding: "8px 10px",
                borderRadius: "6px",
                background: "rgba(220, 53, 69, 0.2)",
                color: "#ff9aa2",
                fontSize: "12px",
              }}
            >
              {error}
            </div>
          )}

          <Focusable style={{ display: "flex", gap: "8px", marginTop: "14px" }}>
            <DialogButton
              style={{ flex: 1 }}
              disabled={saving}
              onClick={handleSave}
            >
              {saving ? "Saving..." : "Update PIN"}
            </DialogButton>
            <DialogButton
              style={{ flex: 0.6 }}
              disabled={saving}
              onClick={() => closeModal?.()}
            >
              Cancel
            </DialogButton>
          </Focusable>
        </DialogControlsSection>
      </DialogBody>
    </ModalRoot>
  );
};

interface RenameHostModalProps {
  currentName: string;
  onSuccess: (status: CrdStatus) => void;
  closeModal?: () => void;
}

export const RenameHostModal: FC<RenameHostModalProps> = ({
  currentName,
  onSuccess,
  closeModal,
}) => {
  const [name, setName] = useState<string>(currentName);
  const [saving, setSaving] = useState<boolean>(false);
  const [error, setError] = useState<string | null>(null);

  const handleSave = async () => {
    setError(null);
    if (!name.trim()) {
      setError("Device name cannot be empty.");
      return;
    }
    setSaving(true);
    try {
      const res = await changeHostName(name.trim());
      if (!res.ok) {
        setError(res.error || "Failed to rename host.");
        return;
      }
      onSuccess(res.status);
      notify("SteamOS Chromoting", `Device renamed to '${name.trim()}'.`);
      closeModal?.();
    } catch (e) {
      setError(`Failed to rename device: ${String(e)}`);
    } finally {
      setSaving(false);
    }
  };

  return (
    <ModalRoot onCancel={closeModal} closeModal={closeModal}>
      <DialogHeader>Change Device Name</DialogHeader>
      <DialogBody>
        <DialogControlsSection>
          <TextField
            label="Display Name in Chrome Remote Desktop"
            value={name}
            disabled={saving}
            onChange={(e) => setName(e.target.value)}
          />
          {error && (
            <div
              style={{
                marginTop: "10px",
                padding: "8px 10px",
                borderRadius: "6px",
                background: "rgba(220, 53, 69, 0.2)",
                color: "#ff9aa2",
                fontSize: "12px",
              }}
            >
              {error}
            </div>
          )}
          <Focusable style={{ display: "flex", gap: "8px", marginTop: "14px" }}>
            <DialogButton
              style={{ flex: 1 }}
              disabled={saving}
              onClick={handleSave}
            >
              {saving ? "Saving..." : "Save Name"}
            </DialogButton>
            <DialogButton
              style={{ flex: 0.6 }}
              disabled={saving}
              onClick={() => closeModal?.()}
            >
              Cancel
            </DialogButton>
          </Focusable>
        </DialogControlsSection>
      </DialogBody>
    </ModalRoot>
  );
};

interface LogsModalProps {
  initialLogs?: string;
  title?: string;
  closeModal?: () => void;
}

export const LogsModal: FC<LogsModalProps> = ({
  initialLogs,
  title = "SteamOS Chromoting Logs",
  closeModal,
}) => {
  const [unit, setUnit] = useState<string>("all");
  const [logs, setLogs] = useState<string>(initialLogs ?? "Loading logs...");
  const [loading, setLoading] = useState<boolean>(!initialLogs);

  const loadLogs = async (selectedUnit: string) => {
    setLoading(true);
    try {
      const res = await getLogs(selectedUnit, 100);
      setLogs(res.logs || "No log output.");
    } catch (e) {
      setLogs(`Failed to load logs: ${String(e)}`);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    if (!initialLogs) {
      loadLogs(unit);
    }
  }, [unit]);

  return (
    <ModalRoot
      onCancel={closeModal}
      closeModal={closeModal}
      bAllowFullSize={true}
    >
      <DialogHeader>{title}</DialogHeader>
      <DialogBody>
        {!initialLogs && (
          <DropdownItem
            label="Service Unit"
            selectedOption={unit}
            rgOptions={[
              { label: "All Services", data: "all" },
              { label: "CRD Host (WebRTC)", data: "host" },
              { label: "Capture & Input Bridge", data: "bridge" },
              { label: "Virtual Display (Xvfb)", data: "xvfb" },
              { label: "Audio Monitor Pipe", data: "audio" },
            ]}
            onChange={(opt) => setUnit(String(opt.data))}
          />
        )}

        <pre
          style={{
            marginTop: "8px",
            maxHeight: "280px",
            overflowY: "auto",
            padding: "10px",
            borderRadius: "6px",
            background: "rgba(0, 0, 0, 0.45)",
            fontSize: "11px",
            lineHeight: 1.35,
            whiteSpace: "pre-wrap",
            wordBreak: "break-word",
            fontFamily: "monospace",
          }}
        >
          {loading ? "Loading journal entries..." : logs}
        </pre>

        <Focusable style={{ display: "flex", gap: "8px", marginTop: "12px" }}>
          {!initialLogs && (
            <DialogButton
              style={{ flex: 1 }}
              disabled={loading}
              onClick={() => loadLogs(unit)}
            >
              Refresh Logs
            </DialogButton>
          )}
          <DialogButton
            style={{ flex: 1 }}
            onClick={() => closeModal?.()}
          >
            Close
          </DialogButton>
        </Focusable>
      </DialogBody>
    </ModalRoot>
  );
};

interface UninstallModalProps {
  onSuccess: (status: CrdStatus) => void;
  closeModal?: () => void;
}

export const UninstallModal: FC<UninstallModalProps> = ({
  onSuccess,
  closeModal,
}) => {
  const [purge, setPurge] = useState<boolean>(false);
  const [busy, setBusy] = useState<boolean>(false);

  const handleConfirm = async () => {
    setBusy(true);
    try {
      const nextStatus = await uninstallServer(purge);
      onSuccess(nextStatus);
      notify(
        "SteamOS Chromoting",
        purge
          ? "Uninstalled server and deleted credentials/settings."
          : "Uninstalled server runtime (kept saved credentials)."
      );
      closeModal?.();
    } catch (e) {
      notify("SteamOS Chromoting", `Uninstall failed: ${String(e)}`);
    } finally {
      setBusy(false);
    }
  };

  return (
    <ConfirmModal
      strTitle="Uninstall SteamOS Chromoting Server"
      strDescription="Stop remote desktop services and remove the rootless server runtime from ~/.local/share/steamos-chromoting?"
      strOKButtonText={busy ? "Uninstalling..." : "Uninstall Server"}
      strCancelButtonText="Cancel"
      bDestructiveWarning={true}
      bOKDisabled={busy}
      bCancelDisabled={busy}
      onOK={handleConfirm}
      onCancel={closeModal}
      closeModal={closeModal}
    >
      <div style={{ marginTop: "10px" }}>
        <ToggleField
          label="Delete Saved Google Credentials & Settings"
          description="Also remove ~/.config/chrome-remote-desktop and ~/.config/steamos-chromoting"
          checked={purge}
          disabled={busy}
          onChange={setPurge}
        />
      </div>
    </ConfirmModal>
  );
};
