#!/usr/bin/env bash
set -euo pipefail

INSTALL_DIR="${STEAMOS_CRD_DIR:-$HOME/.local/share/steamos-chromoting}"
CONFIG_DIR="$HOME/.config/chrome-remote-desktop"
STEAMOS_CRD_BIN="$INSTALL_DIR/bin/steamos-crd"
HEADLESS_URL="https://remotedesktop.google.com/headless"
ACCESS_URL="https://remotedesktop.google.com/access"

detect_ui_backend() {
    if [[ -n "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]]; then
        if command -v kdialog >/dev/null 2>&1; then
            echo "kdialog"
            return
        elif command -v zenity >/dev/null 2>&1; then
            echo "zenity"
            return
        fi
    fi
    echo "tty"
}

UI_BACKEND="$(detect_ui_backend)"

ui_info() {
    local title="$1"
    local msg="$2"
    case "$UI_BACKEND" in
        kdialog)
            kdialog --title "$title" --msgbox "$msg" || true
            ;;
        zenity)
            zenity --info --no-markup --title="$title" --text="$msg" --width=480 || true
            ;;
        *)
            printf '\n=== %s ===\n%s\n\n' "$title" "$msg"
            ;;
    esac
}

ui_error() {
    local title="$1"
    local msg="$2"
    case "$UI_BACKEND" in
        kdialog)
            kdialog --title "$title" --error "$msg" || true
            ;;
        zenity)
            zenity --error --no-markup --title="$title" --text="$msg" --width=500 || true
            ;;
        *)
            printf '\n[ERROR] %s:\n%s\n\n' "$title" "$msg" >&2
            ;;
    esac
}

ui_yesno() {
    local title="$1"
    local msg="$2"
    local yes_label="${3:-Yes}"
    local no_label="${4:-No}"
    case "$UI_BACKEND" in
        kdialog)
            kdialog --title "$title" --yes-label "$yes_label" --no-label "$no_label" --yesno "$msg"
            ;;
        zenity)
            zenity --question --no-markup --title="$title" --ok-label="$yes_label" --cancel-label="$no_label" --text="$msg" --width=480
            ;;
        *)
            local ans=""
            printf '\n=== %s ===\n%s\n' "$title" "$msg"
            if [[ -r /dev/tty ]]; then
                read -r -p "[$yes_label / $no_label] (y/N): " ans </dev/tty || return 1
            else
                read -r -p "[$yes_label / $no_label] (y/N): " ans || return 1
            fi
            [[ "$ans" =~ ^[Yy] ]]
            ;;
    esac
}

ui_input() {
    local title="$1"
    local prompt="$2"
    local default_val="${3:-}"
    case "$UI_BACKEND" in
        kdialog)
            kdialog --title "$title" --inputbox "$prompt" "$default_val"
            ;;
        zenity)
            zenity --entry --title="$title" --text="$prompt" --entry-text="$default_val" --width=520
            ;;
        *)
            local val=""
            printf '\n=== %s ===\n%s\n' "$title" "$prompt" >&2
            if [[ -n "$default_val" ]]; then
                if [[ -r /dev/tty ]]; then
                    read -r -p "[$default_val]: " val </dev/tty || return 1
                else
                    read -r -p "[$default_val]: " val || return 1
                fi
                printf '%s' "${val:-$default_val}"
            else
                if [[ -r /dev/tty ]]; then
                    read -r -p "> " val </dev/tty || return 1
                else
                    read -r -p "> " val || return 1
                fi
                printf '%s' "$val"
            fi
            ;;
    esac
}

ui_pin() {
    local title="$1"
    local prompt="$2"
    case "$UI_BACKEND" in
        kdialog)
            kdialog --title "$title" --password "$prompt"
            ;;
        zenity)
            zenity --entry --hide-text --title="$title" --text="$prompt" --width=440
            ;;
        *)
            local pin=""
            printf '\n=== %s ===\n' "$title" >&2
            if [[ -r /dev/tty ]]; then
                read -r -s -p "$prompt: " pin </dev/tty || return 1
            else
                read -r -s -p "$prompt: " pin || return 1
            fi
            printf '\n' >&2
            printf '%s' "$pin"
            ;;
    esac
}

ui_menu() {
    local title="$1"
    local header="$2"
    shift 2
    case "$UI_BACKEND" in
        kdialog)
            kdialog --title "$title" --menu "$header" "$@"
            ;;
        zenity)
            zenity --list --title="$title" --text="$header" \
                --column="ID" --column="Action" \
                --hide-column=1 --print-column=1 \
                --width=540 --height=380 "$@"
            ;;
        *)
            printf '\n=== %s ===\n%s\n\n' "$title" "$header" >&2
            local -a tags=()
            local -a labels=()
            while [[ $# -ge 2 ]]; do
                tags+=("$1")
                labels+=("$2")
                shift 2
            done
            local i
            for i in "${!tags[@]}"; do
                printf '  %d) %s\n' "$((i + 1))" "${labels[$i]}" >&2
            done
            printf '  q) Quit\n' >&2
            local choice=""
            if [[ -r /dev/tty ]]; then
                read -r -p "Select option: " choice </dev/tty || return 1
            else
                read -r -p "Select option: " choice || return 1
            fi
            if [[ "$choice" =~ ^[0-9]+$ ]] && (( choice >= 1 && choice <= ${#tags[@]} )); then
                printf '%s' "${tags[$((choice - 1))]}"
                return 0
            fi
            return 1
            ;;
    esac
}

open_url() {
    local url="$1"
    if command -v xdg-open >/dev/null 2>&1 && [[ -n "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]]; then
        xdg-open "$url" >/dev/null 2>&1 &
    fi
}

launch_in_terminal() {
    if command -v konsole >/dev/null 2>&1; then
        konsole -e "$@" >/dev/null 2>&1 &
    elif command -v kgx >/dev/null 2>&1; then
        kgx -- "$@" >/dev/null 2>&1 &
    elif command -v xfce4-terminal >/dev/null 2>&1; then
        xfce4-terminal -x "$@" >/dev/null 2>&1 &
    elif command -v xterm >/dev/null 2>&1; then
        xterm -e "$@" >/dev/null 2>&1 &
    else
        ui_error "SteamOS Chromoting" "Could not find a terminal emulator (konsole or xterm) to display output."
    fi
}

get_registered_host_name() {
    local cfg
    cfg="$(find "$CONFIG_DIR" -maxdepth 1 -name 'host#*.json' 2>/dev/null | head -n 1 || true)"
    if [[ -z "$cfg" ]]; then
        return 1
    fi
    if command -v python3 >/dev/null 2>&1; then
        python3 -c 'import json, sys; print(json.load(open(sys.argv[1], encoding="utf-8")).get("host_name", "Registered"))' "$cfg" 2>/dev/null || echo "Registered"
    else
        echo "Registered"
    fi
}

parse_auth_code() {
    local raw="$1"
    raw="${raw//$'\r'/ }"
    raw="${raw//$'\n'/ }"
    local re_dq='--code="([^"]+)"'
    local re_sq="--code='([^']+)'"
    local re_uq='--code=([^[:space:]"'\'']+)'
    if [[ "$raw" =~ $re_dq ]]; then
        printf '%s' "${BASH_REMATCH[1]}"
        return 0
    elif [[ "$raw" =~ $re_sq ]]; then
        printf '%s' "${BASH_REMATCH[1]}"
        return 0
    elif [[ "$raw" =~ $re_uq ]]; then
        printf '%s' "${BASH_REMATCH[1]}"
        return 0
    fi

    raw="$(printf '%s' "$raw" | sed -e 's/^[[:space:]"'\'']*//' -e 's/[[:space:]"'\'']*$//')"
    if [[ -n "$raw" && ! "$raw" =~ [[:space:]] && "$raw" != http*://* && ${#raw} -ge 10 ]]; then
        printf '%s' "$raw"
        return 0
    fi
    return 1
}

parse_redirect_url() {
    local raw="$1"
    raw="${raw//$'\r'/ }"
    raw="${raw//$'\n'/ }"
    local re_dq='--redirect-url="([^"]+)"'
    local re_sq="--redirect-url='([^']+)'"
    local re_uq='--redirect-url=([^[:space:]"'\'']+)'
    if [[ "$raw" =~ $re_dq ]]; then
        printf '%s' "${BASH_REMATCH[1]}"
    elif [[ "$raw" =~ $re_sq ]]; then
        printf '%s' "${BASH_REMATCH[1]}"
    elif [[ "$raw" =~ $re_uq ]]; then
        printf '%s' "${BASH_REMATCH[1]}"
    else
        printf '%s' "https://remotedesktop.google.com/_/oauthredirect"
    fi
}

run_setup_wizard() {
    while true; do
        local intro_msg
        intro_msg="Link your Google account to enable Chrome Remote Desktop:

1. Click \"Open Browser\" to launch:
   $HEADLESS_URL
2. Sign in and click Begin -> Next -> Authorize.
3. Click the Copy button next to \"Debian Linux\".

Open your web browser now?"

        if ui_yesno "SteamOS Chromoting — Setup" "$intro_msg" "Open Browser" "Skip for Later"; then
            open_url "$HEADLESS_URL"
        else
            ui_info "SteamOS Chromoting" "Setup skipped for now.

You can finish linking your Google account anytime by opening \"SteamOS Chromoting\" from the KDE Application Menu, or by running:
  steamos-crd gui"
            return 0
        fi

        local code=""
        local redirect_url="https://remotedesktop.google.com/_/oauthredirect"
        while true; do
            local raw_input=""
            if ! raw_input="$(ui_input "SteamOS Chromoting — Authorization" "After clicking Authorize in your browser, paste the copied \"Debian Linux\" command (or --code) below:")"; then
                if ui_yesno "SteamOS Chromoting" "Cancel Google account registration for now?" "Skip Setup" "Go Back"; then
                    return 0
                fi
                continue
            fi

            if code="$(parse_auth_code "$raw_input")"; then
                redirect_url="$(parse_redirect_url "$raw_input")"
                break
            else
                ui_error "Invalid Authorization Code" "Could not find an authorization code in what you pasted.

Make sure you:
1. Open $HEADLESS_URL
2. Click Begin -> Next -> Authorize
3. Copy the \"Debian Linux\" command and paste it completely."
            fi
        done

        local default_host
        default_host="$(uname -n)"
        local host_name=""
        if ! host_name="$(ui_input "SteamOS Chromoting — Device Name" "Choose a display name for this Steam Deck in Chrome Remote Desktop:" "$default_host")"; then
            host_name="$default_host"
        fi
        host_name="${host_name:-$default_host}"

        local pin=""
        local cancelled_pin=0
        while true; do
            local pin1=""
            if ! pin1="$(ui_pin "SteamOS Chromoting — Set PIN" "Enter a numeric PIN (6 or more digits) to protect remote access:")"; then
                if ui_yesno "SteamOS Chromoting" "Cancel registration?" "Cancel Setup" "Enter PIN"; then
                    cancelled_pin=1
                    break
                fi
                continue
            fi

            if [[ ! "$pin1" =~ ^[0-9]{6,}$ ]]; then
                ui_error "Invalid PIN" "Your PIN must be at least 6 digits and contain numbers (0-9) only."
                continue
            fi

            local pin2=""
            if ! pin2="$(ui_pin "SteamOS Chromoting — Confirm PIN" "Re-enter your PIN to confirm:")"; then
                continue
            fi

            if [[ "$pin1" != "$pin2" ]]; then
                ui_error "PIN Mismatch" "The PINs you entered did not match. Please try again."
                continue
            fi

            pin="$pin1"
            break
        done

        if [[ "$cancelled_pin" -eq 1 ]]; then
            return 0
        fi

        local tmp_log
        tmp_log="$(mktemp "${TMPDIR:-/tmp}/steamos-crd-setup.XXXXXX")"
        local setup_rc=0
        if [[ -t 1 ]]; then
            "$STEAMOS_CRD_BIN" setup \
                "--code=$code" \
                "--pin=$pin" \
                "--name=$host_name" \
                "--redirect-url=$redirect_url" 2>&1 | tee "$tmp_log" || setup_rc=$?
        else
            "$STEAMOS_CRD_BIN" setup \
                "--code=$code" \
                "--pin=$pin" \
                "--name=$host_name" \
                "--redirect-url=$redirect_url" >"$tmp_log" 2>&1 || setup_rc=$?
        fi

        if [[ "$setup_rc" -eq 0 ]]; then
            rm -f "$tmp_log"
            local wayland_note=""
            if [[ "${XDG_SESSION_TYPE:-}" == "wayland" ]]; then
                wayland_note="

Note (Wayland Desktop Mode): If KDE shows a Screen Sharing dialog, select your screen, check \"Allow restoring on future sessions\", and click Share."
            fi
            ui_info "SteamOS Chromoting — Ready!" "Success! \"$host_name\" is registered and running.

Connect from any computer or phone at:
$ACCESS_URL

• Works seamlessly across both Desktop Mode and Gaming Mode.
• Open \"SteamOS Chromoting\" in the KDE Application Menu anytime to check status, re-pair, or uninstall.${wayland_note}"
            return 0
        else
            local err_tail
            err_tail="$(tail -n 12 "$tmp_log" 2>/dev/null || true)"
            rm -f "$tmp_log"
            if ! ui_yesno "Registration Failed" "Google registration failed (OAuth codes are single-use and expire after a few minutes).

Details:
$err_tail

Would you like to generate a fresh code and try again?" "Try Again" "Cancel"; then
                return 1
            fi
        fi
    done
}

cmd_install_wizard() {
    local existing_host=""
    if existing_host="$(get_registered_host_name)"; then
        "$STEAMOS_CRD_BIN" restart || true
        if ui_yesno "SteamOS Chromoting Updated" "SteamOS Chromoting runtime and app launcher have been installed and restarted!

Existing registration found for host: \"$existing_host\"

Keep your existing Google registration, or set up a new one?" "Keep Existing (Done)" "Re-register Host"; then
            return 0
        fi
    fi

    run_setup_wizard
}

cmd_manage() {
    while true; do
        local host_label="Not Registered"
        local is_registered=0
        if host_label="$(get_registered_host_name)"; then
            is_registered=1
        else
            host_label="Not Registered"
        fi

        local host_active="inactive"
        local bridge_active="inactive"
        host_active="$(systemctl --user is-active steamos-crd-host.service 2>/dev/null || true)"
        bridge_active="$(systemctl --user is-active steamos-crd-bridge.service 2>/dev/null || true)"

        local status_summary="Stopped"
        if [[ "$host_active" == "active" && "$bridge_active" == "active" ]]; then
            status_summary="Running"
        elif [[ "$host_active" == "active" || "$bridge_active" == "active" ]]; then
            status_summary="Partially Running (host: $host_active, bridge: $bridge_active)"
        fi

        local header="Host: $host_label   |   Service: $status_summary"
        local -a items=()
        if [[ "$is_registered" -eq 0 ]]; then
            items+=("setup" "Set Up Google Account & PIN (Required)")
        fi

        if [[ "$host_active" == "active" ]]; then
            items+=("stop" "Stop Remote Desktop Services")
            items+=("restart" "Restart Services")
        elif [[ "$is_registered" -eq 1 ]]; then
            items+=("start" "Start Remote Desktop Services")
        fi

        if [[ "$is_registered" -eq 1 ]]; then
            items+=("setup" "Re-pair Google Account or Change PIN")
        fi

        items+=("open_web" "Open Chrome Remote Desktop Web Portal")
        items+=("logs" "View Live Service Logs")
        items+=("update" "Update from Latest GitHub Release")
        items+=("uninstall" "Uninstall SteamOS Chromoting")

        local action=""
        if ! action="$(ui_menu "SteamOS Chromoting" "$header" "${items[@]}")"; then
            break
        fi

        case "$action" in
            start)
                "$STEAMOS_CRD_BIN" start || true
                sleep 1
                ;;
            stop)
                "$STEAMOS_CRD_BIN" stop || true
                sleep 1
                ;;
            restart)
                "$STEAMOS_CRD_BIN" restart || true
                sleep 1
                ;;
            setup)
                run_setup_wizard || true
                ;;
            open_web)
                open_url "$ACCESS_URL"
                ;;
            logs)
                launch_in_terminal "$STEAMOS_CRD_BIN" logs
                ;;
            update)
                launch_in_terminal bash -c '"$1" self-update; echo ""; read -r -p "Press Enter to close..."' -- "$STEAMOS_CRD_BIN"
                break
                ;;
            uninstall)
                if ui_yesno "Uninstall SteamOS Chromoting" "Stop services and uninstall SteamOS Chromoting from this Steam Deck?" "Uninstall" "Cancel"; then
                    if ui_yesno "Host Credentials" "Keep your saved Google host registration (~/.config/chrome-remote-desktop) in case you reinstall later?" "Keep Credentials" "Delete Everything"; then
                        "$STEAMOS_CRD_BIN" uninstall
                    else
                        "$STEAMOS_CRD_BIN" uninstall --purge
                    fi
                    ui_info "SteamOS Chromoting" "SteamOS Chromoting has been uninstalled."
                    break
                fi
                ;;
            *)
                break
                ;;
        esac
    done
}

MODE="${1:-manage}"
shift || true

case "$MODE" in
    install-wizard) cmd_install_wizard "$@" ;;
    setup-wizard)   run_setup_wizard "$@" ;;
    manage|*)       cmd_manage "$@" ;;
esac
