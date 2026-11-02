// SPDX-License-Identifier: GPL-3.0-or-later
//! THROWAWAY PROTOTYPE: can an iced layer share its focus context without a fork?
//! Run: cargo run -p ferroseek-launcher --example activation-context-prototype --locked
//! Enter or click requests a token. Diagnostics never contain token contents.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use iced::futures::channel::oneshot;
use iced::widget::{button, column, container, text};
use iced::{Element, Event, Subscription, Task, keyboard};
use iced_layershell::reexport::{Anchor, KeyboardInteractivity, Layer};
use iced_layershell::settings::LayerShellSettings;
use iced_layershell::{Settings, to_layer_message};
use wayland_client::protocol::{wl_callback, wl_keyboard, wl_pointer, wl_registry, wl_seat, wl_surface};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::xdg::activation::v1::client::{xdg_activation_token_v1, xdg_activation_v1};

type Reply = Arc<Mutex<Option<oneshot::Sender<bool>>>>;

// A sync reply reaches the worker's queue after earlier input on this connection.
// It also wakes blocking_dispatch when the user has otherwise stopped typing.
enum Wake {
    Request(Reply),
    Stop,
}

struct Probe {
    connection: Connection,
    queue: QueueHandle<Worker>,
}

impl Probe {
    fn start(connection: Connection) -> Self {
        let mut events = connection.new_event_queue::<Worker>();
        let queue = events.handle();
        connection.display().get_registry(&queue, ());
        std::thread::spawn(move || {
            let mut worker = Worker::default();
            while !worker.stop {
                if events.blocking_dispatch(&mut worker).is_err() {
                    eprintln!("activation-prototype worker_error=true");
                    break;
                }
            }
            eprintln!("activation-prototype worker_stopped=true");
        });
        Self { connection, queue }
    }

    fn request(&self) -> impl Future<Output = bool> + use<> {
        let (send, receive) = oneshot::channel();
        self.connection.display().sync(&self.queue, Wake::Request(Arc::new(Mutex::new(Some(send)))));
        let flushed = self.connection.flush().is_ok();
        eprintln!("activation-prototype request_queued=true flushed={flushed}");
        async move { flushed && receive.await.unwrap_or(false) }
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        self.connection.display().sync(&self.queue, Wake::Stop);
        let _ = self.connection.flush();
    }
}

struct Context {
    seat: wl_seat::WlSeat,
    surface: wl_surface::WlSurface,
    serial: u32,
    pointer: bool,
}

#[derive(Default)]
struct Worker {
    activation: Option<xdg_activation_v1::XdgActivationV1>,
    keyboards: HashMap<wl_seat::WlSeat, wl_surface::WlSurface>,
    pointers: HashMap<wl_seat::WlSeat, wl_surface::WlSurface>,
    context: Option<Context>,
    stop: bool,
}

impl Worker {
    fn remember(&mut self, seat: &wl_seat::WlSeat, surface: wl_surface::WlSurface, serial: u32, pointer: bool) {
        eprintln!(
            "activation-prototype context=true surface_alive={} seat_alive={} serial={serial} pointer={pointer}",
            surface.is_alive(),
            seat.is_alive()
        );
        self.context = Some(Context { seat: seat.clone(), surface, serial, pointer });
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
        if let wl_registry::Event::Global { name, interface, version } = event {
            match interface.as_str() {
                "wl_seat" => {
                    registry.bind::<wl_seat::WlSeat, _, _>(name, version.min(7), queue, ());
                }
                "xdg_activation_v1" => {
                    state.activation = Some(registry.bind(name, 1, queue, ()));
                    eprintln!("activation-prototype protocol=true");
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for Worker {
    fn event(
        _: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        queue: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities { capabilities: WEnum::Value(capabilities) } = event {
            if capabilities.contains(wl_seat::Capability::Keyboard) {
                seat.get_keyboard(queue, seat.clone());
            }
            if capabilities.contains(wl_seat::Capability::Pointer) {
                seat.get_pointer(queue, seat.clone());
            }
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, wl_seat::WlSeat> for Worker {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        seat: &wl_seat::WlSeat,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_keyboard::Event::Enter { surface, serial, .. } => {
                state.keyboards.insert(seat.clone(), surface.clone());
                state.remember(seat, surface, serial, false);
            }
            wl_keyboard::Event::Key { serial, state: WEnum::Value(wl_keyboard::KeyState::Pressed), .. } => {
                if let Some(surface) = state.keyboards.get(seat).cloned() {
                    state.remember(seat, surface, serial, false);
                }
            }
            wl_keyboard::Event::Leave { .. } => {
                state.keyboards.remove(seat);
                if state.context.as_ref().is_some_and(|context| context.seat == *seat && !context.pointer) {
                    state.context = None;
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, wl_seat::WlSeat> for Worker {
    fn event(
        state: &mut Self,
        _: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        seat: &wl_seat::WlSeat,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Enter { surface, .. } => {
                state.pointers.insert(seat.clone(), surface);
            }
            wl_pointer::Event::Button { serial, state: WEnum::Value(wl_pointer::ButtonState::Pressed), .. } => {
                if let Some(surface) = state.pointers.get(seat).cloned() {
                    state.remember(seat, surface, serial, true);
                }
            }
            wl_pointer::Event::Leave { .. } => {
                state.pointers.remove(seat);
                if state.context.as_ref().is_some_and(|context| context.seat == *seat && context.pointer) {
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
        let Wake::Request(reply) = wake else {
            state.stop = true;
            return;
        };
        let usable = state.context.as_ref().filter(|context| context.surface.is_alive() && context.seat.is_alive());
        eprintln!(
            "activation-prototype barrier=true protocol={} context={}",
            state.activation.is_some(),
            usable.is_some()
        );
        if let (Some(activation), Some(context)) = (&state.activation, usable) {
            let token = activation.get_activation_token(queue, reply.clone());
            token.set_surface(&context.surface);
            token.set_serial(context.serial, &context.seat);
            token.set_app_id("io.github.ferroseek.ActivationPrototypeTarget".into());
            token.commit();
        } else if let Some(send) = reply.lock().unwrap().take() {
            let _ = send.send(false);
        }
    }
}

impl Dispatch<xdg_activation_token_v1::XdgActivationTokenV1, Reply> for Worker {
    fn event(
        _: &mut Self,
        token: &xdg_activation_token_v1::XdgActivationTokenV1,
        event: xdg_activation_token_v1::Event,
        reply: &Reply,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_activation_token_v1::Event::Done { token: value } = event {
            let received = !value.is_empty();
            eprintln!("activation-prototype done=true token_received={received}");
            token.destroy();
            if let Some(send) = reply.lock().unwrap().take() {
                let _ = send.send(received);
            }
        }
    }
}

delegate_noop!(Worker: ignore xdg_activation_v1::XdgActivationV1);

#[to_layer_message]
#[derive(Debug, Clone)]
enum Message {
    Request,
    Finished(bool),
    Quit,
}

struct ViewState {
    probe: Probe,
    status: &'static str,
}

fn update(state: &mut ViewState, message: Message) -> Task<Message> {
    match message {
        Message::Request => {
            state.status = "Waiting for compositor; UI remains interactive";
            Task::perform(state.probe.request(), Message::Finished)
        }
        Message::Finished(received) => {
            state.status = if received {
                "Token received; validity and target focus untested"
            } else {
                "Context, protocol or reply unavailable"
            };
            Task::none()
        }
        Message::Quit => iced::exit(),
        _ => Task::none(),
    }
}

fn view(state: &ViewState) -> Element<'_, Message> {
    container(
        column![
            text("Activation context prototype"),
            text(state.status),
            button("Request token (Enter)").on_press(Message::Request),
            button("Quit (Escape)").on_press(Message::Quit)
        ]
        .spacing(16),
    )
    .padding(24)
    .into()
}

fn main() -> iced_layershell::Result {
    let connection = Connection::connect_to_env().expect("prototype needs a Wayland compositor");
    let probe = Mutex::new(Some(Probe::start(connection.clone())));
    iced_layershell::application(
        move || {
            (
                ViewState {
                    probe: probe.lock().unwrap().take().expect("one boot"),
                    status: "Press Enter or click; inspect stderr",
                },
                Task::none(),
            )
        },
        || "ferroseek-activation-prototype".into(),
        update,
        view,
    )
    .subscription(|_: &ViewState| {
        Subscription::batch([iced::event::listen_with(|event, _, _| match event {
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(keyboard::key::Named::Enter),
                ..
            }) => Some(Message::Request),
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(keyboard::key::Named::Escape),
                ..
            }) => Some(Message::Quit),
            _ => None,
        })])
    })
    .settings(Settings {
        with_connection: Some(connection.into()),
        layer_settings: LayerShellSettings {
            anchor: Anchor::Top,
            layer: Layer::Overlay,
            size: Some((620, 260)),
            margin: (100, 0, 0, 0),
            exclusive_zone: 0,
            keyboard_interactivity: KeyboardInteractivity::Exclusive,
            ..Default::default()
        },
        ..Default::default()
    })
    .run()
}
