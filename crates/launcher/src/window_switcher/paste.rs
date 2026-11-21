// SPDX-License-Identifier: GPL-3.0-or-later
//! Restore clipboard input only after observing the original toplevel.

use std::io::Write;
use std::os::fd::AsFd;
use std::sync::{Arc, mpsc};

use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1 as Manager,
    zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1 as Keyboard,
};

use super::{Catalog, Duration, Entry, Observer, Proxy, QueueHandle, Reply, Seat, Session, Wake, oneshot, wl_registry};
use crate::clipboard_history::publish::Lease;

const TARGET_CHANGED: &str = "The original window does not have focus. Paste the copied text manually.";
const INPUT_UNAVAILABLE: &str = "The desktop could not send Paste. Paste the copied text manually.";
const CLIPBOARD_CHANGED: &str = "The clipboard changed. Copy the entry again before pasting.";

pub struct Capture {
    session: Session,
    first: mpsc::Receiver<Option<Entry>>,
}

impl Capture {
    pub fn start() -> Option<Self> {
        // Reserve a preconnected Wayland socket for the launcher surface.
        if std::env::var_os("WAYLAND_SOCKET").is_some() {
            return None;
        }
        let (send, first) = mpsc::sync_channel(1);
        let session = Session::connect(Observer::Paste(send)).ok()?;
        Some(Self { session, first })
    }

    pub fn finish(self) -> Option<Target> {
        // The origin is meaningful only if it was observed before the launcher maps.
        let origin = self.first.recv_timeout(Duration::from_millis(60)).ok().flatten()?;
        let name = if origin.title.trim().is_empty() { &origin.app_id } else { &origin.title };
        let label = name.split_whitespace().collect::<Vec<_>>().join(" ");
        Some(Target { session: self.session, id: origin.id, label })
    }
}

pub struct Target {
    session: Session,
    id: u64,
    label: String,
}

impl Target {
    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn send(&self, lease: Arc<Lease>) -> impl Future<Output = Result<(), &'static str>> + use<> {
        let (send, receive) = oneshot::channel();
        self.session.wake(Wake::Paste(Request::Prepare { id: self.id, lease }, Some(send).into()));
        async move {
            let runtime = zruntime::SharedRuntime::current().map_err(|_| INPUT_UNAVAILABLE)?;
            runtime
                .timeout(Duration::from_millis(800), receive)
                .await
                .map_err(|_| INPUT_UNAVAILABLE)?
                .map_err(|_| INPUT_UNAVAILABLE)?
        }
    }
}

pub(super) enum Request {
    Prepare { id: u64, lease: Arc<Lease> },
    Send { id: u64, lease: Arc<Lease>, keyboard: PasteKeyboard },
    Submitted,
}

#[derive(Default)]
pub(super) struct Input(Option<(u32, Manager)>);

impl Input {
    pub(super) fn bind(
        &mut self,
        registry: &wl_registry::WlRegistry,
        global: wl_registry::Event,
        queue: &QueueHandle<Catalog>,
    ) {
        if self.0.is_some() {
            return;
        }
        let wl_registry::Event::Global { name, interface, .. } = global else { return };
        if interface == Manager::interface().name {
            self.0 = Some((name, registry.bind(name, 1, queue, ())));
        }
    }

    pub(super) fn remove(&mut self, name: u32) {
        if self.0.as_ref().is_some_and(|(id, _)| *id == name) {
            self.0 = None;
        }
    }
}

pub(super) struct PasteKeyboard {
    proxy: Keyboard,
    control: u32,
    key: u32,
}

impl PasteKeyboard {
    fn new(manager: &Manager, seat: &Seat, queue: &QueueHandle<Catalog>) -> Option<Self> {
        use xkbcommon::xkb;

        // Give the virtual device a known symbol regardless of the user's physical layout.
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let keymap =
            xkb::Keymap::new_from_names(&context, "evdev", "pc105", "us", "", None, xkb::KEYMAP_COMPILE_NO_FLAGS)?;
        let control = 1u32.checked_shl(keymap.mod_get_index(xkb::MOD_NAME_CTRL))?;
        let key = keymap.key_by_name("AB04")?.raw().checked_sub(8)?;
        let mut data = keymap.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1).into_bytes();
        data.push(0);
        let mut file = tempfile::tempfile().ok()?;
        file.write_all(&data).ok()?;
        let proxy = manager.create_virtual_keyboard(seat, queue, ());
        proxy.keymap(1, file.as_fd(), data.len().try_into().ok()?);
        Some(Self { proxy, control, key })
    }

    fn press(&self) {
        self.proxy.modifiers(self.control, 0, 0, 0);
        self.proxy.key(0, self.key, 1);
        self.proxy.key(1, self.key, 0);
        self.proxy.modifiers(0, 0, 0, 0);
    }
}

impl Drop for PasteKeyboard {
    fn drop(&mut self) {
        self.proxy.destroy();
    }
}

impl Catalog {
    pub(super) fn paste_target(&self) -> Option<Entry> {
        if self.unavailable().is_some() || self.paste_keyboard.0.is_none() {
            return None;
        }
        self.sole_active().filter(|entry| !entry.title.trim().is_empty() || !entry.app_id.trim().is_empty()).cloned()
    }

    fn sole_active(&self) -> Option<&Entry> {
        let mut active =
            self.windows.values().filter_map(|window| window.visible.as_ref()).filter(|entry| entry.activated);
        let first = active.next()?;
        active.next().is_none().then_some(first)
    }

    pub(super) fn paste_request(&self, request: &Request, send: &Reply, queue: &QueueHandle<Self>) -> Option<Wake> {
        let send = send.lock().ok().and_then(|mut reply| reply.take())?;
        if send.is_canceled() {
            return None;
        }
        let next = match request {
            Request::Prepare { id, lease } => self.prepare_paste(*id, Arc::clone(lease), queue),
            Request::Send { id, lease, keyboard } => self.send_paste(*id, lease, keyboard),
            Request::Submitted => {
                let _ = send.send(Ok(()));
                return None;
            }
        };
        match next {
            Ok(next) => Some(Wake::Paste(next, Some(send).into())),
            Err(error) => {
                let _ = send.send(Err(error));
                None
            }
        }
    }

    fn prepare_paste(&self, id: u64, lease: Arc<Lease>, queue: &QueueHandle<Self>) -> Result<Request, &'static str> {
        self.verify_target(id, &lease)?;
        let (_, manager) = self.paste_keyboard.0.as_ref().ok_or(INPUT_UNAVAILABLE)?;
        let seat = self.seats.values().next().ok_or(INPUT_UNAVAILABLE)?;
        let keyboard = PasteKeyboard::new(manager, seat, queue).ok_or(INPUT_UNAVAILABLE)?;
        Ok(Request::Send { id, lease, keyboard })
    }

    fn send_paste(&self, id: u64, lease: &Lease, keyboard: &PasteKeyboard) -> Result<Request, &'static str> {
        self.verify_target(id, lease)?;
        keyboard.press();
        Ok(Request::Submitted)
    }

    fn verify_target(&self, id: u64, lease: &Lease) -> Result<(), &'static str> {
        if !lease.is_current() {
            return Err(CLIPBOARD_CHANGED);
        }
        if self.unavailable().is_some() || !self.sole_active().is_some_and(|entry| entry.id == id) {
            return Err(TARGET_CHANGED);
        }
        Ok(())
    }
}

wayland_client::delegate_noop!(Catalog: ignore Manager);
wayland_client::delegate_noop!(Catalog: ignore Keyboard);
