// SPDX-License-Identifier: GPL-3.0-or-later
//! Fresh activation context from the same Wayland client as the layer surface.
//!
//! Another event queue receives focus/input serials without reading key contents.
//! Iced remains the sole socket reader on this connection.
//! A display sync both wakes the worker and orders input before a token request.

use std::collections::HashMap;
use std::future::poll_fn;
use std::pin::pin;
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::Duration;

use iced::futures::{channel::oneshot, executor::block_on, future::select};
use wayland_client::backend::WaylandError;
use wayland_client::protocol::{wl_callback, wl_keyboard, wl_pointer, wl_registry, wl_seat, wl_surface};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::xdg::activation::v1::client::{xdg_activation_token_v1, xdg_activation_v1};

type Reply = Arc<Mutex<Option<oneshot::Sender<Option<String>>>>>;

#[derive(Clone, Default)]
pub struct Activation(Arc<Mutex<Option<Arc<Client>>>>);

struct Client {
    connection: Connection,
    queue: QueueHandle<Worker>,
    stop: oneshot::Sender<()>,
}

impl Activation {
    pub fn attach(&self, connection: Connection, material: crate::material::Material, output: crate::output::Observer) {
        let mut events = connection.new_event_queue::<Worker>();
        let queue = events.handle();
        connection.display().get_registry(&queue, ());
        let (stop, stopped) = oneshot::channel();
        let started = std::thread::Builder::new().name("activation".into()).spawn(move || {
            let mut worker = Worker::with_material(material);
            worker.output = output;
            // Reading here can drain the socket while iced is waiting for
            // readability, leaving its startup events queued without a wake.
            let dispatch = poll_fn(|cx| {
                if events.poll_dispatch_pending(cx, &mut worker).is_ready() || !can_flush(events.flush()) {
                    return Poll::Ready(());
                }
                Poll::Pending
            });
            let _ = block_on(select(pin!(dispatch), stopped));
            worker.material.disconnect();
            worker.output.disconnect();
        });
        if started.is_ok()
            && let Ok(mut client) = self.0.lock()
        {
            *client = Some(Arc::new(Client { connection, queue, stop }));
        }
    }

    pub async fn token(&self, app_id: String) -> Option<String> {
        let client = self.0.lock().ok()?.clone()?;
        if client.stop.is_canceled() {
            return None;
        }
        let (send, receive) = oneshot::channel();
        client.connection.display().sync(&client.queue, Request { app_id, reply: Arc::new(Mutex::new(Some(send))) });
        if !can_flush(client.connection.flush()) {
            return None;
        }
        // This awaits only a protocol reply, never a zbus future on iced's executor.
        let runtime = zruntime::SharedRuntime::current().ok()?;
        runtime.timeout(Duration::from_secs(1), receive).await.ok()?.ok()?.filter(|token| !token.is_empty())
    }
}

fn can_flush(result: Result<(), WaylandError>) -> bool {
    match result {
        Ok(()) => true,
        // Iced's Wayland event source will flush once the socket is writable.
        Err(WaylandError::Io(error)) => error.kind() == std::io::ErrorKind::WouldBlock,
        Err(_) => false,
    }
}

struct Request {
    app_id: String,
    reply: Reply,
}

struct Seat {
    proxy: wl_seat::WlSeat,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    pointer: Option<wl_pointer::WlPointer>,
    keyboard_focus: Option<wl_surface::WlSurface>,
    pointer_focus: Option<wl_surface::WlSurface>,
}

impl Drop for Seat {
    fn drop(&mut self) {
        if let Some(keyboard) = self.keyboard.take() {
            keyboard.release();
        }
        if let Some(pointer) = self.pointer.take() {
            pointer.release();
        }
        self.proxy.release();
    }
}

struct Context {
    seat: u32,
    surface: wl_surface::WlSurface,
    serial: u32,
    pointer: bool,
}

#[derive(Default)]
pub(super) struct Worker {
    pub(super) material: crate::material::Material,
    pub(super) output: crate::output::Observer,
    activation: Option<(u32, xdg_activation_v1::XdgActivationV1)>,
    seats: HashMap<u32, Seat>,
    context: Option<Context>,
    pending: Vec<xdg_activation_token_v1::XdgActivationTokenV1>,
}

impl Worker {
    pub(super) fn with_material(material: crate::material::Material) -> Self {
        Self {
            material,
            output: crate::output::Observer::default(),
            activation: None,
            seats: HashMap::new(),
            context: None,
            pending: Vec::new(),
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        for token in self.pending.drain(..) {
            token.destroy();
        }
        if let Some((_, activation)) = self.activation.take() {
            activation.destroy();
        }
    }
}

fn reply(reply: &Reply, value: Option<String>) {
    if let Ok(mut send) = reply.lock()
        && let Some(send) = send.take()
    {
        let _ = send.send(value);
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for Worker {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        queue: &QueueHandle<Self>,
    ) {
        state.material.registry(registry, &event, queue);
        state.output.registry(registry, &event, queue);
        match event {
            wl_registry::Event::Global { name, interface, version } if interface == "wl_seat" && version >= 5 => {
                let proxy = registry.bind(name, version.min(7), queue, name);
                state.seats.insert(
                    name,
                    Seat { proxy, keyboard: None, pointer: None, keyboard_focus: None, pointer_focus: None },
                );
            }
            wl_registry::Event::Global { name, interface, .. } if interface == "xdg_activation_v1" => {
                state.activation = Some((name, registry.bind(name, 1, queue, ())));
            }
            wl_registry::Event::GlobalRemove { name } => {
                state.seats.remove(&name);
                if state.context.as_ref().is_some_and(|context| context.seat == name) {
                    state.context = None;
                }
                if state.activation.as_ref().is_some_and(|(global, _)| *global == name)
                    && let Some((_, activation)) = state.activation.take()
                {
                    activation.destroy();
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_seat::WlSeat, u32> for Worker {
    fn event(
        state: &mut Self,
        _: &wl_seat::WlSeat,
        event: wl_seat::Event,
        name: &u32,
        _: &Connection,
        queue: &QueueHandle<Self>,
    ) {
        let Some(seat) = state.seats.get_mut(name) else { return };
        if let wl_seat::Event::Capabilities { capabilities: WEnum::Value(capabilities) } = event {
            if capabilities.contains(wl_seat::Capability::Keyboard) {
                if seat.keyboard.is_none() {
                    seat.keyboard = Some(seat.proxy.get_keyboard(queue, *name));
                }
            } else {
                if let Some(keyboard) = seat.keyboard.take() {
                    keyboard.release();
                }
                seat.keyboard_focus = None;
                if state.context.as_ref().is_some_and(|context| context.seat == *name && !context.pointer) {
                    state.context = None;
                }
            }
            if capabilities.contains(wl_seat::Capability::Pointer) {
                if seat.pointer.is_none() {
                    seat.pointer = Some(seat.proxy.get_pointer(queue, *name));
                }
            } else {
                if let Some(pointer) = seat.pointer.take() {
                    pointer.release();
                }
                seat.pointer_focus = None;
                if state.context.as_ref().is_some_and(|context| context.seat == *name && context.pointer) {
                    state.context = None;
                }
            }
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, u32> for Worker {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        name: &u32,
        _: &Connection,
        queue: &QueueHandle<Self>,
    ) {
        let Some(seat) = state.seats.get_mut(name) else { return };
        match event {
            wl_keyboard::Event::Enter { surface, serial, .. } => {
                state.material.surface(surface.clone(), queue);
                state.output.surface(surface.clone(), queue);
                crate::timing::event("keyboard_focus_enter");
                seat.keyboard_focus = Some(surface.clone());
                state.context = Some(Context { seat: *name, surface, serial, pointer: false });
            }
            wl_keyboard::Event::Key { serial, .. } => {
                if let Some(surface) = seat.keyboard_focus.clone() {
                    state.context = Some(Context { seat: *name, surface, serial, pointer: false });
                }
            }
            wl_keyboard::Event::Leave { .. } => {
                crate::timing::event("keyboard_focus_leave");
                seat.keyboard_focus = None;
                if state.context.as_ref().is_some_and(|context| context.seat == *name && !context.pointer) {
                    state.context = None;
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, u32> for Worker {
    fn event(
        state: &mut Self,
        _: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        name: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(seat) = state.seats.get_mut(name) else { return };
        match event {
            wl_pointer::Event::Enter { surface, .. } => seat.pointer_focus = Some(surface),
            wl_pointer::Event::Button { serial, .. } => {
                if let Some(surface) = seat.pointer_focus.clone() {
                    state.context = Some(Context { seat: *name, surface, serial, pointer: true });
                }
            }
            wl_pointer::Event::Leave { .. } => {
                seat.pointer_focus = None;
                if state.context.as_ref().is_some_and(|context| context.seat == *name && context.pointer) {
                    state.context = None;
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_callback::WlCallback, Request> for Worker {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        _: wl_callback::Event,
        request: &Request,
        _: &Connection,
        queue: &QueueHandle<Self>,
    ) {
        let Request { app_id, reply: send } = request;
        state.pending.retain(|token| {
            let canceled = token.data::<Reply>().is_some_and(|reply| {
                reply.lock().map_or(true, |send| send.as_ref().is_none_or(oneshot::Sender::is_canceled))
            });
            if canceled {
                token.destroy();
            }
            !canceled
        });
        if send.lock().map_or(true, |send| send.as_ref().is_none_or(oneshot::Sender::is_canceled)) {
            return;
        }
        if let (Some((_, activation)), Some(context)) = (&state.activation, &state.context)
            && let Some(seat) = state.seats.get(&context.seat)
            && context.surface.is_alive()
        {
            let token = activation.get_activation_token(queue, send.clone());
            token.set_serial(context.serial, &seat.proxy);
            token.set_surface(&context.surface);
            if !app_id.is_empty() {
                token.set_app_id(app_id.clone());
            }
            token.commit();
            state.pending.push(token);
        } else {
            reply(send, None);
        }
    }
}

impl Dispatch<xdg_activation_token_v1::XdgActivationTokenV1, Reply> for Worker {
    fn event(
        state: &mut Self,
        token: &xdg_activation_token_v1::XdgActivationTokenV1,
        event: xdg_activation_token_v1::Event,
        send: &Reply,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_activation_token_v1::Event::Done { token: value } = event {
            state.pending.retain(|pending| pending != token);
            token.destroy();
            reply(send, Some(value));
        }
    }
}

delegate_noop!(Worker: ignore xdg_activation_v1::XdgActivationV1);

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::Shutdown;
    use std::os::unix::net::UnixStream;
    use std::sync::mpsc;

    pub(crate) enum Mode {
        EmptyRegistry,
        NoSeat,
        Stall,
        Disconnect,
    }

    pub(crate) struct Server {
        socket: UnixStream,
        thread: Option<std::thread::JoinHandle<()>>,
        reader: Option<std::thread::JoinHandle<()>>,
    }

    impl Drop for Server {
        fn drop(&mut self) {
            let _ = self.socket.shutdown(Shutdown::Both);
            let result = self.thread.take().unwrap().join();
            let read_result = self.reader.take().unwrap().join();
            if !std::thread::panicking() {
                assert!(result.is_ok(), "protocol peer failed");
                assert!(read_result.is_ok(), "protocol reader failed");
            }
        }
    }

    // Minimal core-protocol peer: no globals, callback replies or a stalled peer.
    // This mocks the compositor boundary, not the activation worker's internals.
    pub(crate) fn compositor(mode: Mode) -> (Activation, Server, mpsc::Receiver<()>) {
        let (client, mut peer) = UnixStream::pair().unwrap();
        let socket = peer.try_clone().unwrap();
        let (send, seen) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            if matches!(mode, Mode::Disconnect) {
                let _ = peer.shutdown(Shutdown::Both);
                return;
            }
            loop {
                let mut header = [0; 8];
                if peer.read_exact(&mut header).is_err() {
                    break;
                }
                let object = u32::from_ne_bytes(header[..4].try_into().unwrap());
                let header = u32::from_ne_bytes(header[4..].try_into().unwrap());
                let size = (header >> 16) as usize;
                assert!((8..=256).contains(&size));
                let mut body = vec![0; size - 8];
                if peer.read_exact(&mut body).is_err() {
                    break;
                }
                if object != 1 {
                    assert!(matches!(mode, Mode::NoSeat)); // registry bind / manager destroy
                    continue;
                }
                match header & 0xffff {
                    1 => {
                        // get_registry: optionally advertise activation, but no seat
                        if matches!(mode, Mode::NoSeat) {
                            let registry = u32::from_ne_bytes(body[..4].try_into().unwrap());
                            if peer.write_all(&activation_global(registry)).is_err() {
                                break;
                            }
                        }
                    }
                    0 => {
                        // sync: callback.done followed by display.delete_id
                        let _ = send.send(());
                        if matches!(mode, Mode::Stall) {
                            continue;
                        }
                        let id = u32::from_ne_bytes(body[..4].try_into().unwrap());
                        let words = [id, 12 << 16, 0, 1, (12 << 16) | 1, id];
                        let response: Vec<_> = words.iter().flat_map(|word| word.to_ne_bytes()).collect();
                        if peer.write_all(&response).is_err() {
                            break;
                        }
                    }
                    opcode => panic!("unexpected core request {opcode}"),
                }
            }
        });
        let connection = Connection::from_socket(client).unwrap();
        let activation = Activation::default();
        activation.attach(connection.clone(), crate::material::Material::default(), crate::output::Observer::default());
        let reader = std::thread::spawn(move || read_display(connection));
        (activation, Server { socket, thread: Some(thread), reader: Some(reader) }, seen)
    }

    // Read the test connection on behalf of iced.
    fn read_display(connection: Connection) {
        let mut events = connection.new_event_queue();
        let mut worker = Worker::default();
        while events.blocking_dispatch(&mut worker).is_ok() {}
    }

    fn activation_global(registry: u32) -> Vec<u8> {
        let name = b"xdg_activation_v1\0";
        let mut global = 7_u32.to_ne_bytes().to_vec();
        global.extend_from_slice(&(name.len() as u32).to_ne_bytes());
        global.extend_from_slice(name);
        global.resize(global.len().next_multiple_of(4), 0);
        global.extend_from_slice(&1_u32.to_ne_bytes());
        let mut message = registry.to_ne_bytes().to_vec();
        message.extend_from_slice(&((global.len() as u32 + 8) << 16).to_ne_bytes());
        message.extend(global);
        message
    }

    #[test]
    fn activation_queue_waits_for_the_primary_wayland_reader() {
        let (client, mut peer) = UnixStream::pair().unwrap();
        let connection = Connection::from_socket(client).unwrap();
        let activation = Activation::default();
        activation.attach(connection.clone(), crate::material::Material::default(), crate::output::Observer::default());
        connection.flush().unwrap();

        peer.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
        let mut registry_request = [0; 12];
        peer.read_exact(&mut registry_request).unwrap();
        let registry = u32::from_ne_bytes(registry_request[8..].try_into().unwrap());
        peer.write_all(&activation_global(registry)).unwrap();

        let mut bind_header = [0; 8];
        peer.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
        let error = peer.read_exact(&mut bind_header).expect_err("activation must leave socket reads to iced");
        assert!(matches!(error.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut));

        connection.prepare_read().unwrap().read().unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
        peer.read_exact(&mut bind_header).expect("activation binds after iced reads the registry event");
        assert_eq!(u32::from_ne_bytes(bind_header[..4].try_into().unwrap()), registry);
    }

    fn request(activation: &Activation) -> Option<String> {
        let runtime = zruntime::SharedRuntime::current().unwrap();
        iced::futures::executor::block_on(
            runtime.timeout(Duration::from_secs(3), activation.token("org.ferroseek.Test".into())),
        )
        .expect("activation must not hang when a compositor cannot answer")
    }

    #[test]
    fn missing_activation_protocol_falls_back_without_context() {
        let (activation, _server, seen) = compositor(Mode::EmptyRegistry);
        assert!(request(&activation).is_none());
        seen.recv_timeout(Duration::from_secs(1)).expect("request reached the real protocol peer");
    }

    #[test]
    fn silent_compositor_has_a_bounded_context_wait() {
        let (activation, _server, seen) = compositor(Mode::Stall);
        assert!(request(&activation).is_none());
        seen.recv_timeout(Duration::from_secs(1)).expect("peer held the requested barrier open");
    }

    #[test]
    fn activation_protocol_without_a_focused_seat_does_not_invent_context() {
        let (activation, _server, seen) = compositor(Mode::NoSeat);
        assert!(request(&activation).is_none());
        seen.recv_timeout(Duration::from_secs(1)).expect("context lookup reached the compositor");
    }

    #[test]
    fn disconnected_compositor_does_not_confirm_context() {
        let (activation, _server, _) = compositor(Mode::Disconnect);
        assert!(request(&activation).is_none());
    }
}
