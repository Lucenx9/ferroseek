// SPDX-License-Identifier: GPL-3.0-or-later
//! Fresh activation context from the same Wayland client as the layer surface.
//!
//! Another event queue receives focus/input serials without reading key contents.
//! A display sync both wakes the worker and orders input before a token request.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iced::futures::channel::oneshot;
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
    alive: Arc<AtomicBool>,
}

impl Activation {
    pub fn attach(&self, connection: Connection) {
        let mut events = connection.new_event_queue::<Worker>();
        let queue = events.handle();
        connection.display().get_registry(&queue, ());
        let alive = Arc::new(AtomicBool::new(true));
        let running = alive.clone();
        let started = std::thread::Builder::new().name("activation".into()).spawn(move || {
            let mut worker = Worker::default();
            while !worker.stop && events.blocking_dispatch(&mut worker).is_ok() {}
            running.store(false, Ordering::Release);
        });
        if started.is_ok()
            && let Ok(mut client) = self.0.lock()
        {
            *client = Some(Arc::new(Client { connection, queue, alive }));
        }
    }

    pub async fn token(&self, app_id: String) -> Option<String> {
        let client = self.0.lock().ok()?.clone()?;
        if !client.alive.load(Ordering::Acquire) {
            return None;
        }
        let (send, receive) = oneshot::channel();
        client
            .connection
            .display()
            .sync(&client.queue, Wake::Request { app_id, reply: Arc::new(Mutex::new(Some(send))) });
        match client.connection.flush() {
            Ok(()) => {}
            Err(WaylandError::Io(error)) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => return None,
        }
        // This awaits only a protocol reply, never a zbus future on iced's executor.
        let runtime = zruntime::SharedRuntime::current().ok()?;
        runtime.timeout(Duration::from_secs(1), receive).await.ok()?.ok()?.filter(|token| !token.is_empty())
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.connection.display().sync(&self.queue, Wake::Stop);
        let _ = self.connection.flush();
    }
}

enum Wake {
    Request { app_id: String, reply: Reply },
    Stop,
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
struct Worker {
    activation: Option<(u32, xdg_activation_v1::XdgActivationV1)>,
    seats: HashMap<u32, Seat>,
    context: Option<Context>,
    pending: Vec<xdg_activation_token_v1::XdgActivationTokenV1>,
    stop: bool,
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
        _: &QueueHandle<Self>,
    ) {
        let Some(seat) = state.seats.get_mut(name) else { return };
        match event {
            wl_keyboard::Event::Enter { surface, serial, .. } => {
                seat.keyboard_focus = Some(surface.clone());
                state.context = Some(Context { seat: *name, surface, serial, pointer: false });
            }
            wl_keyboard::Event::Key { serial, .. } => {
                if let Some(surface) = seat.keyboard_focus.clone() {
                    state.context = Some(Context { seat: *name, surface, serial, pointer: false });
                }
            }
            wl_keyboard::Event::Leave { .. } => {
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

impl Dispatch<wl_callback::WlCallback, Wake> for Worker {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        _: wl_callback::Event,
        wake: &Wake,
        _: &Connection,
        queue: &QueueHandle<Self>,
    ) {
        let Wake::Request { app_id, reply: send } = wake else {
            state.stop = true;
            return;
        };
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
