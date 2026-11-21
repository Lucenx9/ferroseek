// SPDX-License-Identifier: GPL-3.0-or-later
//! A short-lived catalog of switchable Wayland windows.

mod paste;

pub use paste::{Capture as PasteCapture, Target as PasteTarget};

use std::collections::BTreeMap;
use std::time::Duration;

use iced::futures::channel::{mpsc, oneshot};
use wayland_client::backend::ObjectId;
use wayland_client::protocol::{wl_callback, wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, delegate_noop};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1 as handle, zwlr_foreign_toplevel_manager_v1 as manager,
};

type Window = handle::ZwlrForeignToplevelHandleV1;
type Manager = manager::ZwlrForeignToplevelManagerV1;
type Seat = wl_seat::WlSeat;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Entry {
    pub id: u64,
    pub title: String,
    pub app_id: String,
    pub minimized: bool,
    pub maximized: bool,
    pub activated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Switch,
    Maximize(bool),
    Minimize(bool),
}

impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Self::Switch => "Switch",
            Self::Maximize(true) => "Maximize",
            Self::Maximize(false) => "Restore size",
            Self::Minimize(true) => "Minimize",
            Self::Minimize(false) => "Unminimize",
        }
    }

    pub fn confirmation(self) -> &'static str {
        match self {
            Self::Switch => "Switched window",
            Self::Maximize(true) => "Window maximized",
            Self::Maximize(false) => "Window size restored",
            Self::Minimize(true) => "Window minimized",
            Self::Minimize(false) => "Window unminimized",
        }
    }

    fn reached(self, entry: &Entry) -> bool {
        match self {
            Self::Switch => false,
            Self::Maximize(value) => entry.maximized == value,
            Self::Minimize(value) => entry.minimized == value,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Update {
    Windows(Vec<Entry>),
    Unavailable(&'static str),
}

pub struct Session {
    connection: Connection,
    queue: QueueHandle<Catalog>,
}

impl Session {
    pub fn start() -> Result<(Self, mpsc::UnboundedReceiver<Update>), &'static str> {
        let (send, receive) = mpsc::unbounded();
        Self::connect(Observer::Windows(send)).map(|session| (session, receive))
    }

    fn connect(observer: Observer) -> Result<Self, &'static str> {
        let connection = Connection::connect_to_env().map_err(|_| DISCONNECTED)?;
        let mut events = connection.new_event_queue::<Catalog>();
        let queue = events.handle();
        connection.display().get_registry(&queue, ());
        std::thread::Builder::new()
            .name("window-switcher".into())
            .spawn(move || {
                let mut catalog =
                    Catalog { observe_paste_target: matches!(observer, Observer::Paste(_)), ..Catalog::default() };
                if let (Err(message), Observer::Windows(send)) = (catalog.follow(&mut events, &observer), &observer) {
                    let _ = send.unbounded_send(Update::Unavailable(message));
                }
                catalog.stop();
                let _ = events.flush();
            })
            .map_err(|_| DISCONNECTED)?;
        Ok(Self { connection, queue })
    }

    pub fn perform(&self, id: u64, action: Action) -> impl Future<Output = Result<(), &'static str>> + use<> {
        let (send, receive) = oneshot::channel();
        self.wake(Wake::Request(id, action, Some(send).into()));
        async move {
            let runtime = zruntime::SharedRuntime::current().map_err(|_| DISCONNECTED)?;
            runtime
                .timeout(Duration::from_secs(2), receive)
                .await
                .map_err(|_| "The desktop did not confirm the request. Check the window and try again.")?
                .map_err(|_| DISCONNECTED)?
        }
    }

    fn wake(&self, wake: Wake) {
        self.connection.display().sync(&self.queue, wake);
        let _ = self.connection.flush();
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.wake(Wake::Stop);
    }
}

const DISCONNECTED: &str = "Could not read open windows. Go back and reopen Switch windows.";
const CLOSED: &str = "That window closed. Choose another window.";
type Reply = std::sync::Mutex<Option<oneshot::Sender<Result<(), &'static str>>>>;

enum Wake {
    Stop,
    Request(u64, Action, Reply),
    Sent(u64, Reply),
    Paste(paste::Request, Reply),
}

enum Observer {
    Windows(mpsc::UnboundedSender<Update>),
    Paste(std::sync::mpsc::SyncSender<Option<Entry>>),
}

fn reply(send: &Reply, result: Result<(), &'static str>) {
    if let Some(send) = send.lock().ok().and_then(|mut slot| slot.take()) {
        let _ = send.send(result);
    }
}

struct Toplevel {
    window: Window,
    draft: Entry,
    visible: Option<Entry>,
}

struct Pending {
    id: u64,
    action: Action,
    reply: oneshot::Sender<Result<(), &'static str>>,
}

#[derive(Default)]
struct Catalog {
    manager: Option<(u32, Manager)>,
    seats: BTreeMap<u32, Seat>,
    windows: BTreeMap<u64, Toplevel>,
    handles: std::collections::HashMap<ObjectId, u64>,
    next_id: u64,
    pending: Option<Pending>,
    observe_paste_target: bool,
    paste_keyboard: paste::Input,
    changed: bool,
    stopping: bool,
    finished: bool,
}

impl Catalog {
    fn follow(
        &mut self,
        events: &mut wayland_client::EventQueue<Self>,
        observer: &Observer,
    ) -> Result<(), &'static str> {
        // The first sync discovers globals. The second waits for each window's initial properties.
        events.roundtrip(self).and_then(|_| events.roundtrip(self)).map_err(|_| DISCONNECTED)?;
        if let Observer::Paste(send) = observer {
            send.send(self.paste_target()).map_err(|_| DISCONNECTED)?;
        }
        self.changed = true;
        while !self.finished {
            if !self.stopping
                && let Observer::Windows(send) = observer
            {
                self.publish(send)?;
            }
            events.blocking_dispatch(self).map_err(|_| DISCONNECTED)?;
        }
        if self.stopping { Ok(()) } else { Err(DISCONNECTED) }
    }

    fn publish(&mut self, send: &mpsc::UnboundedSender<Update>) -> Result<(), &'static str> {
        if let Some(message) = self.unavailable() {
            return Err(message);
        }
        if std::mem::take(&mut self.changed) {
            let entries = self.windows.values().filter_map(|window| window.visible.clone()).collect();
            send.unbounded_send(Update::Windows(entries)).map_err(|_| DISCONNECTED)?;
        }
        Ok(())
    }

    fn unavailable(&self) -> Option<&'static str> {
        if self.manager.is_none() {
            Some("Window switching is unavailable on this desktop.")
        } else if self.seats.len() != 1 {
            Some("Window switching needs a desktop session with one input seat.")
        } else {
            None
        }
    }

    fn stop(&mut self) {
        if self.stopping {
            return;
        }
        self.stopping = true;
        self.handles.clear();
        for (_, window) in std::mem::take(&mut self.windows) {
            window.window.destroy();
        }
        if let Some((_, manager)) = &self.manager {
            manager.stop();
        } else {
            self.finished = true;
        }
    }

    fn request(&self, id: u64, action: Action) -> Result<(), &'static str> {
        if let Some(message) = self.unavailable() {
            return Err(message);
        }
        let window = self.windows.get(&id).ok_or(CLOSED)?;
        let window = &window.window;
        match action {
            Action::Switch => window.activate(self.seats.values().next().expect("one input seat")),
            Action::Maximize(true) => window.set_maximized(),
            Action::Maximize(false) => window.unset_maximized(),
            Action::Minimize(true) => window.set_minimized(),
            Action::Minimize(false) => window.unset_minimized(),
        }
        Ok(())
    }
    fn add(&mut self, window: Window) {
        if self.stopping {
            window.destroy();
            return;
        }
        self.next_id += 1;
        let id = self.next_id;
        self.handles.insert(window.id(), id);
        self.windows.insert(id, Toplevel { window, draft: Entry { id, ..Entry::default() }, visible: None });
    }

    fn close(&mut self, window: &Window, id: u64) {
        window.destroy();
        self.handles.remove(&window.id());
        self.windows.remove(&id);
        self.changed = true;
    }
}

impl Catalog {
    fn registry_event(
        &mut self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        queue: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global { name, interface, version }
                if interface == Manager::interface().name && self.manager.is_none() =>
            {
                self.manager = Some((name, registry.bind(name, version.min(3), queue, ())));
            }
            wl_registry::Event::Global { name, interface, version } if interface == Seat::interface().name => {
                self.seats.insert(name, registry.bind(name, version.min(5), queue, ()));
                self.changed = true;
            }
            global @ wl_registry::Event::Global { .. } if self.observe_paste_target => {
                self.paste_keyboard.bind(registry, global, queue);
            }
            wl_registry::Event::GlobalRemove { name } => self.remove_global(name),
            _ => {}
        }
    }

    fn confirm_change(&mut self) {
        let Some(pending) = &self.pending else { return };
        let result = match self.windows.get(&pending.id) {
            Some(window) => window.visible.as_ref().filter(|entry| pending.action.reached(entry)).map(|_| Ok(())),
            None => Some(Err(CLOSED)),
        };
        if let Some(result) = result {
            let _ = self.pending.take().expect("pending action").reply.send(result);
        }
    }

    fn remove_global(&mut self, name: u32) {
        self.paste_keyboard.remove(name);
        if let Some(seat) = self.seats.remove(&name) {
            if seat.version() >= 5 {
                seat.release();
            }
            self.changed = true;
        }
        if self.manager.as_ref().is_some_and(|(id, _)| *id == name) {
            self.finished = true;
        }
    }

    fn manager_event(&mut self, event: manager::Event) {
        match event {
            manager::Event::Toplevel { toplevel } => self.add(toplevel),
            manager::Event::Finished => self.finished = true,
            _ => {}
        }
    }

    fn window_event(&mut self, window: &Window, event: handle::Event) {
        let Some(&id) = self.handles.get(&window.id()) else { return };
        match event {
            handle::Event::Closed => self.close(window, id),
            event => {
                if let Some(record) = self.windows.get_mut(&id) {
                    self.changed |= record.update(event);
                }
            }
        }
        self.confirm_change();
    }

    fn wake(&mut self, wake: &Wake, conn: &Connection, queue: &QueueHandle<Self>) {
        let next = match wake {
            Wake::Stop => {
                self.stop();
                None
            }
            Wake::Sent(id, send) => {
                reply(send, self.windows.contains_key(id).then_some(()).ok_or(CLOSED));
                None
            }
            Wake::Paste(request, send) => self.paste_request(request, send, queue),
            Wake::Request(id, action, send) => self.window_request(*id, *action, send),
        };
        if let Some(next) = next {
            conn.display().sync(queue, next);
        }
    }

    fn window_request(&mut self, id: u64, action: Action, send: &Reply) -> Option<Wake> {
        let send = send.lock().ok().and_then(|mut slot| slot.take())?;
        if send.is_canceled() || self.stopping {
            return None;
        }
        match self.request(id, action) {
            // Confirm receipt before the overlay releases focus.
            Ok(()) if action == Action::Switch => Some(Wake::Sent(id, Some(send).into())),
            Ok(()) => {
                self.pending = Some(Pending { id, action, reply: send });
                self.confirm_change();
                None
            }
            Err(error) => {
                let _ = send.send(Err(error));
                None
            }
        }
    }
}

impl Toplevel {
    fn update(&mut self, event: handle::Event) -> bool {
        match event {
            handle::Event::Title { title } => self.draft.title = title,
            handle::Event::AppId { app_id } => self.draft.app_id = app_id,
            handle::Event::State { state } => {
                let (states, _) = state.as_chunks::<4>();
                let has = |state: handle::State| states.iter().any(|bytes| u32::from_ne_bytes(*bytes) == state as u32);
                self.draft.minimized = has(handle::State::Minimized);
                self.draft.maximized = has(handle::State::Maximized);
                self.draft.activated = has(handle::State::Activated);
            }
            handle::Event::Done if self.visible.as_ref() != Some(&self.draft) => {
                self.visible = Some(self.draft.clone());
                return true;
            }
            _ => {}
        }
        false
    }
}

// The protocol always supplies the same dispatch context. Pass only the
// portions each catalog handler uses, and declare constructor events here.
macro_rules! catalog_dispatch {
    ($object:ty, $data:ty,
        |$state:ident, $proxy:pat_param, $event:pat_param, $value:pat_param, $conn:pat_param, $queue:pat_param|
        $forward:expr $(; $opcode:path => $child:ty)?
    ) => {
        impl Dispatch<$object, $data> for Catalog {
            fn event(
                $state: &mut Self,
                $proxy: &$object,
                $event: <$object as Proxy>::Event,
                $value: &$data,
                $conn: &Connection,
                $queue: &QueueHandle<Self>,
            ) {
                $forward
            }

            $(wayland_client::event_created_child!(Catalog, $object, [$opcode => ($child, ())]);)?
        }
    };
}

delegate_noop!(Catalog: ignore Seat);
catalog_dispatch!(wl_registry::WlRegistry, (), |state, registry, event, _, _, queue| state
    .registry_event(registry, event, queue));
catalog_dispatch!(Manager, (), |state, _, event, _, _, _| state.manager_event(event);
    manager::EVT_TOPLEVEL_OPCODE => Window);
catalog_dispatch!(Window, (), |state, window, event, _, _, _| state.window_event(window, event));
catalog_dispatch!(wl_callback::WlCallback, Wake, |state, _, _, wake, conn, queue| state.wake(wake, conn, queue));
