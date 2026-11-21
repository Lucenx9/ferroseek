// SPDX-License-Identifier: GPL-3.0-or-later
//! Send one native paste shortcut through KWin's keyboard-symbol protocol.

use std::fs::File;
use std::os::unix::fs::FileExt;

use wayland_client::backend::ObjectId;
use wayland_client::globals::Global;
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::{wl_keyboard, wl_seat};
use wayland_client::{Proxy, QueueHandle, WEnum};
use wayland_protocols_plasma::keystate::client::org_kde_kwin_keystate as keystate;
use xkbcommon::xkb;
use xkbcommon::xkb::keysyms::{KEY_Control_L, KEY_v};

use super::{Catalog, Seat};

// From plasma-wayland-protocols d50aab9be549633bf0c3a4747e09e0147b0ff758.
// Use the crate's bindings once they include keyboard_keysym (version 6).
mod fake_input {
    #[allow(
        clippy::single_component_path_imports,
        reason = "Generated protocol code imports super::wayland_client"
    )]
    use wayland_client;
    use wayland_client::backend as wayland_backend;

    mod __interfaces {
        use super::wayland_backend;

        wayland_scanner::generate_interfaces!("src/window_switcher/plasma-fake-input-v6.xml");
    }

    use self::__interfaces::*;

    wayland_scanner::generate_client_code!("src/window_switcher/plasma-fake-input-v6.xml");
}

type FakeInput = fake_input::org_kde_kwin_fake_input::OrgKdeKwinFakeInput;
pub(super) type KeyState = keystate::OrgKdeKwinKeystate;
pub(super) type SeatKeyboard = wl_keyboard::WlKeyboard;

// The five modifier keys report depressed, latched and locked states in v5.
const MODIFIERS: u8 = 0b1111_1000;

struct Modifiers {
    name: u32,
    proxy: KeyState,
    observed: u8,
    held: u8,
}

impl Modifiers {
    fn ready(&self) -> bool {
        self.observed == MODIFIERS
    }

    fn observe(&mut self, key: u32, state: u32) {
        let Some(bit) = 1u8.checked_shl(key).filter(|bit| MODIFIERS & bit != 0) else { return };
        self.observed |= bit;
        self.held &= !bit;
        if state != keystate::State::Unlocked as u32 {
            self.held |= bit;
        }
    }
}

impl Drop for Modifiers {
    fn drop(&mut self) {
        self.proxy.destroy();
    }
}

struct SeatLayout {
    seat: ObjectId,
    keyboard: SeatKeyboard,
    maps_shortcut: bool,
}

impl Drop for SeatLayout {
    fn drop(&mut self) {
        self.keyboard.release();
    }
}

fn read_keymap(map: File, size: u32) -> Option<xkb::Keymap> {
    if size > 1024 * 1024 || !map.metadata().ok()?.is_file() {
        return None;
    }
    let mut text = vec![0; size as usize];
    map.read_exact_at(&mut text, 0).ok()?;
    if text.pop()? != 0 || text.contains(&0) {
        return None;
    }
    let text = String::from_utf8(text).ok()?;
    let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
    xkb::Keymap::new_from_string(&context, text, xkb::KEYMAP_FORMAT_TEXT_V1, xkb::KEYMAP_COMPILE_NO_FLAGS)
}

fn maps_paste_shortcut(keymap: &xkb::Keymap) -> bool {
    // An unfocused wl_keyboard reports the map, but not the active group.
    // KWin replaces the map for a missing symbol, which can drop Ctrl.
    let maps_symbol = |group, symbol| {
        (keymap.min_keycode().raw()..keymap.max_keycode().raw()).any(|code| {
            keymap.key_get_syms_by_level(xkb::Keycode::new(code), group, 0).iter().any(|key| key.raw() == symbol)
        })
    };
    (0..keymap.num_layouts()).all(|group| maps_symbol(group, KEY_Control_L) && maps_symbol(group, KEY_v))
}

#[derive(Default)]
pub(super) struct Input {
    device: Option<(u32, WlRegistry)>,
    modifiers: Option<Modifiers>,
    layout: Option<SeatLayout>,
}

impl Input {
    pub(super) fn discover(&mut self, registry: &WlRegistry, global: &Global, queue: &QueueHandle<Catalog>) {
        if global.interface == FakeInput::interface().name && global.version >= 6 {
            self.device.get_or_insert_with(|| (global.name, registry.clone()));
        } else if global.interface == KeyState::interface().name && global.version >= 5 {
            self.modifiers.get_or_insert_with(|| {
                let proxy: KeyState = registry.bind(global.name, 5, queue, ());
                proxy.fetchStates();
                Modifiers { name: global.name, proxy, observed: 0, held: 0 }
            });
        }
    }

    pub(super) fn remove(&mut self, name: u32) {
        if self.device.as_ref().is_some_and(|(id, _)| *id == name) {
            self.device = None;
        }
        if self.modifiers.as_ref().is_some_and(|modifiers| modifiers.name == name) {
            self.modifiers = None;
        }
    }

    pub(super) fn available(&self) -> bool {
        self.device.is_some()
            && self.modifiers.as_ref().is_some_and(Modifiers::ready)
            && self.layout.as_ref().is_some_and(|layout| layout.maps_shortcut)
    }

    pub(super) fn can_type(&self) -> bool {
        self.available() && self.modifiers.as_ref().is_some_and(|modifiers| modifiers.held == 0)
    }

    pub(super) fn observe(&mut self, event: keystate::Event) {
        if let keystate::Event::StateChanged { key, state } = event
            && let Some(modifiers) = &mut self.modifiers
        {
            modifiers.observe(key, state);
        }
    }

    pub(super) fn seat_changed(&mut self, seat: &Seat, event: wl_seat::Event, queue: &QueueHandle<Catalog>) {
        let wl_seat::Event::Capabilities { capabilities: WEnum::Value(capabilities) } = event else { return };
        if !capabilities.contains(wl_seat::Capability::Keyboard) {
            self.remove_seat(seat);
        } else if self.device.is_some() && seat.version() >= 3 {
            self.layout.get_or_insert_with(|| SeatLayout {
                seat: seat.id(),
                keyboard: seat.get_keyboard(queue, ()),
                maps_shortcut: false,
            });
        }
    }

    pub(super) fn remove_seat(&mut self, seat: &Seat) {
        if self.layout.as_ref().is_some_and(|layout| layout.seat == seat.id()) {
            self.layout = None;
        }
    }

    pub(super) fn keymap_changed(&mut self, keyboard: &SeatKeyboard, event: wl_keyboard::Event) {
        let Some(layout) = self.layout.as_mut().filter(|layout| layout.keyboard == *keyboard) else { return };
        if let wl_keyboard::Event::Keymap { format, fd, size } = event {
            layout.maps_shortcut = format == WEnum::Value(wl_keyboard::KeymapFormat::XkbV1)
                && read_keymap(File::from(fd), size).as_ref().is_some_and(maps_paste_shortcut);
        }
    }

    pub(super) fn keyboard(&self, queue: &QueueHandle<Catalog>) -> Option<Keyboard> {
        let (name, registry) = self.device.as_ref()?;
        let proxy: FakeInput = registry.bind(*name, 6, queue, ());
        proxy.authenticate("FerroSeek".into(), "Paste selected text into the original window".into());
        Some(Keyboard(proxy))
    }
}

pub(super) struct Keyboard(FakeInput);

impl Keyboard {
    pub(super) fn press(&self) {
        self.0.keyboard_keysym(KEY_Control_L, 1);
        self.0.keyboard_keysym(KEY_v, 1);
        self.0.keyboard_keysym(KEY_v, 0);
        self.0.keyboard_keysym(KEY_Control_L, 0);
    }
}

impl Drop for Keyboard {
    fn drop(&mut self) {
        self.0.destroy();
    }
}

wayland_client::delegate_noop!(Catalog: ignore FakeInput);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paste_requires_ctrl_and_v_without_synthetic_keymap_changes() {
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        for (layout, variant, expected) in [("us", "", true), ("us", "dvorak", true), ("ru", "", false)] {
            let keymap = xkb::Keymap::new_from_names(
                &context,
                "evdev",
                "pc105",
                layout,
                variant,
                None,
                xkb::KEYMAP_COMPILE_NO_FLAGS,
            )
            .expect("test keyboard layout");
            assert_eq!(maps_paste_shortcut(&keymap), expected, "{layout}({variant})");
        }
    }
}
