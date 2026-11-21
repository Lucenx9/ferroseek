// SPDX-License-Identifier: GPL-3.0-or-later
//! Restore clipboard input only after observing the original toplevel.

use std::io::Write;
use std::os::fd::AsFd;
use std::sync::{Arc, mpsc};

use wayland_client::Connection;
use wayland_client::globals::Global;
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1 as Manager,
    zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1 as Keyboard,
};

use super::{
    Action, Catalog, Duration, Entry, Observer, Proxy, QueueHandle, Reply, Seat, Session, Wake, oneshot, plasma_keys,
    wl_registry,
};
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
        self.request(Request::Prepare(Attempt { id: self.id, lease }))
    }

    pub fn restore_focus(&self, lease: Arc<Lease>) -> impl Future<Output = Result<(), &'static str>> + use<> {
        self.request(Request::Restore(Attempt { id: self.id, lease }))
    }

    fn request(&self, request: Request) -> impl Future<Output = Result<(), &'static str>> + use<> {
        let (send, receive) = oneshot::channel();
        self.session.wake(Wake::Paste(request, Some(send).into()));
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

#[derive(Clone)]
pub(super) struct Attempt {
    id: u64,
    lease: Arc<Lease>,
}

pub(super) enum Request {
    Restore(Attempt),
    Prepare(Attempt),
    Send(Attempt, PasteKeyboard),
    Submitted,
}

#[derive(Default)]
pub(super) struct Input {
    virtual_manager: Option<(u32, Manager)>,
    plasma: plasma_keys::Input,
    waiting: Option<Wake>,
}

impl Input {
    pub(super) fn bind(&mut self, registry: &wl_registry::WlRegistry, global: &Global, queue: &QueueHandle<Catalog>) {
        if self.virtual_manager.is_none() && global.interface == Manager::interface().name {
            self.virtual_manager = Some((global.name, registry.bind(global.name, 1, queue, ())));
        }
        self.plasma.discover(registry, global, queue);
    }

    pub(super) fn remove(&mut self, name: u32) {
        if self.virtual_manager.as_ref().is_some_and(|(id, _)| *id == name) {
            self.virtual_manager = None;
        }
        self.plasma.remove(name);
    }

    pub(super) fn remove_seat(&mut self, seat: &Seat) {
        self.plasma.remove_seat(seat);
    }

    fn available(&self) -> bool {
        self.virtual_manager.is_some() || self.plasma.available()
    }

    fn can_type(&self) -> bool {
        self.virtual_manager.is_some() || self.plasma.can_type()
    }

    fn keyboard(&self, seat: &Seat, queue: &QueueHandle<Catalog>) -> Option<PasteKeyboard> {
        if let Some((_, manager)) = &self.virtual_manager {
            return VirtualKeyboard::new(manager, seat, queue).map(PasteKeyboard::Virtual);
        }
        self.plasma.keyboard(queue).map(PasteKeyboard::Plasma)
    }
}

pub(super) enum PasteKeyboard {
    Virtual(VirtualKeyboard),
    Plasma(plasma_keys::Keyboard),
}

impl PasteKeyboard {
    fn press(&self) {
        match self {
            Self::Virtual(keyboard) => keyboard.press(),
            Self::Plasma(keyboard) => keyboard.press(),
        }
    }
}

pub(super) struct VirtualKeyboard {
    proxy: Keyboard,
    control: u32,
    key: u32,
}

impl VirtualKeyboard {
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

impl Drop for VirtualKeyboard {
    fn drop(&mut self) {
        self.proxy.destroy();
    }
}

impl Catalog {
    pub(super) fn paste_seat_changed(&mut self, seat: &Seat, event: <Seat as Proxy>::Event, queue: &QueueHandle<Self>) {
        if self.observe_paste_target {
            self.paste_keyboard.plasma.seat_changed(seat, event, queue);
        }
    }

    pub(super) fn paste_keymap_changed(
        &mut self,
        keyboard: &plasma_keys::SeatKeyboard,
        event: <plasma_keys::SeatKeyboard as Proxy>::Event,
    ) {
        self.paste_keyboard.plasma.keymap_changed(keyboard, event);
    }

    pub(super) fn paste_target(&self) -> Option<Entry> {
        if self.unavailable().is_some() || !self.paste_keyboard.available() {
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

    pub(super) fn paste_modifiers_changed(
        &mut self,
        event: <plasma_keys::KeyState as Proxy>::Event,
        conn: &Connection,
        queue: &QueueHandle<Self>,
    ) {
        self.paste_keyboard.plasma.observe(event);
        if self.paste_keyboard.can_type()
            && let Some(waiting) = self.paste_keyboard.waiting.take()
        {
            conn.display().sync(queue, waiting);
        }
    }

    pub(super) fn paste_request(&mut self, request: &Request, send: &Reply, queue: &QueueHandle<Self>) -> Option<Wake> {
        let send = send.lock().ok().and_then(|mut reply| reply.take())?;
        if send.is_canceled() {
            return None;
        }
        let next = match request {
            Request::Restore(attempt) => {
                let _ = send.send(self.restore_paste_focus(attempt));
                return None;
            }
            Request::Prepare(attempt) => return self.prepare_paste(attempt, send, queue),
            Request::Send(attempt, keyboard) => self.send_paste(attempt, keyboard),
            Request::Submitted => {
                let _ = send.send(Ok(()));
                return None;
            }
        };
        wake_or_reject(next, send)
    }

    fn restore_paste_focus(&self, attempt: &Attempt) -> Result<(), &'static str> {
        if !matches!(self.manager, Some((_, super::Manager::Plasma))) {
            return Ok(());
        }
        if !attempt.lease.is_current() {
            return Err(CLIPBOARD_CHANGED);
        }
        // KWin can keep the layer focused after it gives up interactivity.
        // Restore the captured window only while no application has focus.
        if self.windows.values().any(|window| window.draft.activated) {
            return self.verify_target(attempt);
        }
        self.request(attempt.id, Action::Switch).map_err(|_| TARGET_CHANGED)
    }

    fn prepare_paste(&mut self, attempt: &Attempt, reply: PasteReply, queue: &QueueHandle<Self>) -> Option<Wake> {
        if let Err(error) = self.verify_target(attempt) {
            return wake_or_reject(Err(error), reply);
        }
        if !self.paste_keyboard.available() {
            return wake_or_reject(Err(INPUT_UNAVAILABLE), reply);
        }
        if !self.paste_keyboard.can_type() {
            // KWin shares modifier state with the physical keyboard. Wait for
            // Alt+Enter (or a sticky modifier) to clear before creating a device.
            self.paste_keyboard.waiting = Some(Wake::Paste(Request::Prepare(attempt.clone()), Some(reply).into()));
            return None;
        }
        if !attempt.lease.confirm_current() {
            return wake_or_reject(Err(CLIPBOARD_CHANGED), reply);
        }
        let seat = self.seats.values().next().expect("validated only input seat");
        let next = self.paste_keyboard.keyboard(seat, queue).ok_or(INPUT_UNAVAILABLE);
        wake_or_reject(next.map(|keyboard| Request::Send(attempt.clone(), keyboard)), reply)
    }

    fn send_paste(&self, attempt: &Attempt, keyboard: &PasteKeyboard) -> Result<Request, &'static str> {
        self.verify_target(attempt)?;
        if !self.paste_keyboard.can_type() {
            return Err("A modifier key is active. Release it or paste the copied text manually.");
        }
        keyboard.press();
        Ok(Request::Submitted)
    }

    fn verify_target(&self, attempt: &Attempt) -> Result<(), &'static str> {
        if !attempt.lease.is_current() {
            return Err(CLIPBOARD_CHANGED);
        }
        if self.unavailable().is_some() || !self.sole_active().is_some_and(|entry| entry.id == attempt.id) {
            return Err(TARGET_CHANGED);
        }
        Ok(())
    }
}

type PasteReply = oneshot::Sender<Result<(), &'static str>>;

fn wake_or_reject(result: Result<Request, &'static str>, send: PasteReply) -> Option<Wake> {
    match result {
        Ok(next) => Some(Wake::Paste(next, Some(send).into())),
        Err(error) => {
            let _ = send.send(Err(error));
            None
        }
    }
}

wayland_client::delegate_noop!(Catalog: ignore Manager);
wayland_client::delegate_noop!(Catalog: ignore Keyboard);
