use anyhow::{Context, Result};
use evdev::uinput::{VirtualDevice, VirtualDeviceBuilder};
use evdev::{
    AbsInfo, AttributeSet, EventType, InputEvent, AbsoluteAxisType, Key, PropType,
    RelativeAxisType, UinputAbsSetup,
};
use tracing::{info, warn};
use x11rb::protocol::xtest;
use x11rb::wrapper::ConnectionExt as _;

pub struct UinputInjector {
    pointer: VirtualDevice,
    keyboard: VirtualDevice,
    ctrl_l: bool,
    ctrl_r: bool,
    alt_l: bool,
    alt_r: bool,
}

impl UinputInjector {
    pub fn new() -> Result<Self> {
        // 1. Virtual absolute tablet/direct pointer (0..32767) with BTN_TOOL_PEN so Linux kernel
        //    `joydev` blacklists it (never creating `/dev/input/js*`), while libinput in Gamescope,
        //    KWin Wayland, and Xorg maps coordinates 1:1 across the active display.
        let mut ptr_buttons = AttributeSet::<Key>::new();
        ptr_buttons.insert(Key::BTN_LEFT);
        ptr_buttons.insert(Key::BTN_RIGHT);
        ptr_buttons.insert(Key::BTN_MIDDLE);
        ptr_buttons.insert(Key::BTN_SIDE);
        ptr_buttons.insert(Key::BTN_EXTRA);

        let mut ptr_rels = AttributeSet::<RelativeAxisType>::new();
        ptr_rels.insert(RelativeAxisType::REL_WHEEL);
        ptr_rels.insert(RelativeAxisType::REL_HWHEEL);

        let mut ptr_props = AttributeSet::<PropType>::new();
        ptr_props.insert(PropType::POINTER);

        let abs_setup = AbsInfo::new(16384, 0, 32767, 0, 0, 1);
        let abs_x = UinputAbsSetup::new(AbsoluteAxisType::ABS_X, abs_setup);
        let abs_y = UinputAbsSetup::new(AbsoluteAxisType::ABS_Y, abs_setup);

        let pointer = VirtualDeviceBuilder::new()
            .context("Failed to open /dev/uinput for virtual pointer")?
            .name("SteamOS-CRD Virtual Pointer")
            .with_properties(&ptr_props)?
            .with_keys(&ptr_buttons)?
            .with_relative_axes(&ptr_rels)?
            .with_absolute_axis(&abs_x)?
            .with_absolute_axis(&abs_y)?
            .build()
            .context("Failed to build virtual pointer uinput device")?;

        // 2. Virtual keyboard supporting all standard evdev keycodes (1..=248).
        let mut kbd_keys = AttributeSet::<Key>::new();
        for code in 1u16..=248u16 {
            kbd_keys.insert(Key::new(code));
        }

        let keyboard = VirtualDeviceBuilder::new()
            .context("Failed to open /dev/uinput for virtual keyboard")?
            .name("SteamOS-CRD Virtual Keyboard")
            .with_keys(&kbd_keys)?
            .build()
            .context("Failed to build virtual keyboard uinput device")?;

        info!("Initialized /dev/uinput virtual pointer and keyboard");

        Ok(Self {
            pointer,
            keyboard,
            ctrl_l: false,
            ctrl_r: false,
            alt_l: false,
            alt_r: false,
        })
    }

    pub fn move_pointer_normalized(&mut self, norm_x: f64, norm_y: f64) {
        let ax = (norm_x.clamp(0.0, 1.0) * 32767.0).round() as i32;
        let ay = (norm_y.clamp(0.0, 1.0) * 32767.0).round() as i32;
        let events = [
            InputEvent::new(EventType::ABSOLUTE, AbsoluteAxisType::ABS_X.0, ax),
            InputEvent::new(EventType::ABSOLUTE, AbsoluteAxisType::ABS_Y.0, ay),
        ];
        let _ = self.pointer.emit(&events);
    }

    pub fn handle_x11_button(&mut self, detail: u8, pressed: bool) {
        match detail {
            1 => self.emit_button(Key::BTN_LEFT, pressed),
            2 => self.emit_button(Key::BTN_MIDDLE, pressed),
            3 => self.emit_button(Key::BTN_RIGHT, pressed),
            4 if pressed => self.emit_wheel(RelativeAxisType::REL_WHEEL, 1),
            5 if pressed => self.emit_wheel(RelativeAxisType::REL_WHEEL, -1),
            6 if pressed => self.emit_wheel(RelativeAxisType::REL_HWHEEL, -1),
            7 if pressed => self.emit_wheel(RelativeAxisType::REL_HWHEEL, 1),
            8 => self.emit_button(Key::BTN_SIDE, pressed),
            9 => self.emit_button(Key::BTN_EXTRA, pressed),
            _ => {}
        }
    }

    fn emit_button(&mut self, btn: Key, pressed: bool) {
        let ev = InputEvent::new(EventType::KEY, btn.code(), if pressed { 1 } else { 0 });
        let _ = self.pointer.emit(&[ev]);
    }

    fn emit_wheel(&mut self, axis: RelativeAxisType, delta: i32) {
        let ev = InputEvent::new(EventType::RELATIVE, axis.0, delta);
        let _ = self.pointer.emit(&[ev]);
    }

    /// Translates X11 keycode (evdev + 8) and intercepts Gaming Mode hardware chords:
    /// - `Ctrl + Alt + S` -> STEAM button
    /// - `Ctrl + Alt + Q` -> Quick Access Menu (`...`)
    pub fn handle_x11_key(&mut self, x11_keycode: u8, pressed: bool, in_gaming_mode: bool) {
        if x11_keycode < 9 {
            return;
        }
        let evdev_code = (x11_keycode - 8) as u16;
        let key = Key::new(evdev_code);

        match key {
            Key::KEY_LEFTCTRL => self.ctrl_l = pressed,
            Key::KEY_RIGHTCTRL => self.ctrl_r = pressed,
            Key::KEY_LEFTALT => self.alt_l = pressed,
            Key::KEY_RIGHTALT => self.alt_r = pressed,
            _ => {}
        }

        let ctrl_held = self.ctrl_l || self.ctrl_r;
        let alt_held = self.alt_l || self.alt_r;

        if in_gaming_mode && pressed && ctrl_held && alt_held {
            if key == Key::KEY_S {
                info!("Chord Ctrl+Alt+S triggered -> opening STEAM menu/overlay");
                self.release_modifiers();
                trigger_gamescope_steam_chord(false);
                return;
            } else if key == Key::KEY_Q {
                info!("Chord Ctrl+Alt+Q triggered -> opening Quick Access Menu (...)");
                self.release_modifiers();
                trigger_gamescope_steam_chord(true);
                return;
            }
        }

        let ev = InputEvent::new(EventType::KEY, evdev_code, if pressed { 1 } else { 0 });
        let _ = self.keyboard.emit(&[ev]);
    }

    fn release_modifiers(&mut self) {
        let mut rel = Vec::new();
        if self.ctrl_l {
            rel.push(InputEvent::new(EventType::KEY, Key::KEY_LEFTCTRL.code(), 0));
            self.ctrl_l = false;
        }
        if self.ctrl_r {
            rel.push(InputEvent::new(EventType::KEY, Key::KEY_RIGHTCTRL.code(), 0));
            self.ctrl_r = false;
        }
        if self.alt_l {
            rel.push(InputEvent::new(EventType::KEY, Key::KEY_LEFTALT.code(), 0));
            self.alt_l = false;
        }
        if self.alt_r {
            rel.push(InputEvent::new(EventType::KEY, Key::KEY_RIGHTALT.code(), 0));
            self.alt_r = false;
        }
        if !rel.is_empty() {
            let _ = self.keyboard.emit(&rel);
        }
    }
}

/// Sends `Ctrl+1` (STEAM menu) or `Ctrl+2` (Quick Access Menu `...`) directly to `:0`
/// (Steam GamepadUI's dedicated Xwayland instance), which works even when a fullscreen game
/// owns keyboard focus on `:1`.
fn trigger_gamescope_steam_chord(is_qam: bool) {
    let target_x11_key: u8 = if is_qam {
        11 // KEY_2 (3) + 8 = 11
    } else {
        10 // KEY_1 (2) + 8 = 10
    };
    let ctrl_x11_key: u8 = 37; // KEY_LEFTCTRL (29) + 8 = 37

    match x11rb::connect(Some(":0")) {
        Ok((conn, _)) => {
            let _ = xtest::fake_input(&conn, x11rb::protocol::xproto::KEY_PRESS_EVENT, ctrl_x11_key, 0, 0, 0, 0, 0);
            let _ = xtest::fake_input(&conn, x11rb::protocol::xproto::KEY_PRESS_EVENT, target_x11_key, 0, 0, 0, 0, 0);
            let _ = conn.sync();
            std::thread::sleep(std::time::Duration::from_millis(60));
            let _ = xtest::fake_input(&conn, x11rb::protocol::xproto::KEY_RELEASE_EVENT, target_x11_key, 0, 0, 0, 0, 0);
            let _ = xtest::fake_input(&conn, x11rb::protocol::xproto::KEY_RELEASE_EVENT, ctrl_x11_key, 0, 0, 0, 0, 0);
            let _ = conn.sync();
        }
        Err(e) => {
            warn!("Failed to connect to Gamescope :0 for Steam chord: {e}");
        }
    }
}
