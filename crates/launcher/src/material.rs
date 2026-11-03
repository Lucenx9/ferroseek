// SPDX-License-Identifier: GPL-3.0-or-later
//! Optional compositor blur, limited to the launcher's rounded surface.
//!
//! The activation queue discovers the surface without raw-window handles. UI
//! resize requests update the region before iced commits its next frame.

pub mod renderer;

use std::sync::{Arc, Mutex};

use iced::futures::channel::mpsc;
use wayland_client::protocol::{wl_compositor, wl_registry, wl_surface};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols::ext::background_effect::v1::client::{
    ext_background_effect_manager_v1 as ext_manager, ext_background_effect_surface_v1 as ext_surface,
};
use wayland_protocols_plasma::blur::client::{
    org_kde_kwin_blur as kde_surface, org_kde_kwin_blur_manager as kde_manager,
};

use crate::activation::Worker;

pub const RADIUS: u32 = 14;

#[derive(Clone)]
pub struct Material(Arc<Mutex<State>>);

struct State {
    allowed: bool,
    size: (u32, u32),
    updates: mpsc::UnboundedSender<bool>,
    enabled: bool,
    compositor: Option<(u32, wl_compositor::WlCompositor)>,
    standard: Option<(u32, ext_manager::ExtBackgroundEffectManagerV1)>,
    legacy: Option<(u32, kde_manager::OrgKdeKwinBlurManager)>,
    capabilities: bool,
    surface: Option<wl_surface::WlSurface>,
    queue: Option<QueueHandle<Worker>>,
    effect: Option<Effect>,
}

enum Effect {
    Standard(ext_surface::ExtBackgroundEffectSurfaceV1),
    Legacy(kde_surface::OrgKdeKwinBlur),
}

impl Drop for Effect {
    fn drop(&mut self) {
        match self {
            Self::Standard(effect) => effect.destroy(),
            Self::Legacy(effect) => effect.release(),
        }
    }
}

impl Default for Material {
    fn default() -> Self {
        Self::new(false, (0, 0)).0
    }
}

impl Material {
    pub fn new(allowed: bool, size: (u32, u32)) -> (Self, mpsc::UnboundedReceiver<bool>) {
        let (updates, events) = mpsc::unbounded();
        let state = State {
            allowed,
            size,
            updates,
            enabled: false,
            compositor: None,
            standard: None,
            legacy: None,
            capabilities: false,
            surface: None,
            queue: None,
            effect: None,
        };
        (Self(Arc::new(Mutex::new(state))), events)
    }

    pub fn resize(&self, size: (u32, u32)) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if state.size != size {
            state.size = size;
            if matches!(state.effect, Some(Effect::Legacy(_))) {
                // Legacy KWin marks surface blur dirty when an object is
                // assigned, not when that object's region alone changes.
                state.effect = None;
                state.refresh();
            } else {
                state.region();
            }
        }
    }

    pub fn surface(&self, surface: wl_surface::WlSurface, queue: &QueueHandle<Worker>) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if state.surface.as_ref() != Some(&surface) {
            state.effect = None;
            state.surface = Some(surface);
        }
        state.queue = Some(queue.clone());
        state.refresh();
    }

    pub fn disconnect(&self) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        state.effect = None;
        state.notify(false);
    }

    pub fn registry(
        &self,
        registry: &wl_registry::WlRegistry,
        event: &wl_registry::Event,
        queue: &QueueHandle<Worker>,
    ) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if !state.allowed {
            return;
        }
        match event {
            wl_registry::Event::Global { name, interface, .. } if interface == "wl_compositor" => {
                state.compositor = Some((*name, registry.bind(*name, 1, queue, ())));
            }
            wl_registry::Event::Global { name, interface, .. } if interface == "ext_background_effect_manager_v1" => {
                state.standard = Some((*name, registry.bind(*name, 1, queue, ())));
            }
            wl_registry::Event::Global { name, interface, .. } if interface == "org_kde_kwin_blur_manager" => {
                state.legacy = Some((*name, registry.bind(*name, 1, queue, ())));
            }
            wl_registry::Event::GlobalRemove { name } => {
                if state.compositor.as_ref().is_some_and(|(id, _)| id == name) {
                    state.compositor = None;
                }
                if state.standard.as_ref().is_some_and(|(id, _)| id == name) {
                    if let Some((_, manager)) = state.standard.take() {
                        manager.destroy();
                    }
                    state.capabilities = false;
                }
                if state.legacy.as_ref().is_some_and(|(id, _)| id == name) {
                    state.legacy = None;
                }
            }
            _ => {}
        }
        state.refresh();
    }
}

impl State {
    fn notify(&mut self, enabled: bool) {
        if enabled != self.enabled {
            self.enabled = enabled;
            let _ = self.updates.unbounded_send(enabled);
        }
    }

    fn refresh(&mut self) {
        let standard = self.standard.is_some();
        // An advertised standard manager owns capability policy, including
        // withdrawal. Do not bypass it through the legacy extension.
        let supported = if standard { self.capabilities } else { self.legacy.is_some() };
        let enabled = self.allowed
            && supported
            && self.compositor.is_some()
            && self.surface.as_ref().is_some_and(Proxy::is_alive);
        if !enabled {
            self.effect = None;
        } else if self.effect.as_ref().is_none_or(|effect| matches!(effect, Effect::Standard(_)) != standard) {
            self.effect = None;
            if let (Some(surface), Some(queue)) = (&self.surface, &self.queue) {
                self.effect = if let Some((_, manager)) = &self.standard {
                    Some(Effect::Standard(manager.get_background_effect(surface, queue, ())))
                } else {
                    self.legacy.as_ref().map(|(_, manager)| Effect::Legacy(manager.create(surface, queue, ())))
                };
                self.region();
            }
        }
        self.notify(enabled && self.effect.is_some());
    }

    fn region(&self) {
        let (Some(effect), Some((_, compositor)), Some(queue)) = (&self.effect, &self.compositor, &self.queue) else {
            return;
        };
        let region = compositor.create_region(queue, ());
        for (x, y, width, height) in rounded_region(self.size) {
            region.add(x, y, width, height);
        }
        match effect {
            Effect::Standard(effect) => effect.set_blur_region(Some(&region)),
            Effect::Legacy(effect) => {
                effect.set_region(Some(&region));
                effect.commit();
            }
        }
        region.destroy();
        // The wl_surface commit belongs to iced: committing here could publish
        // a partially prepared buffer or resize on its other event queue.
    }
}

fn rounded_region((width, height): (u32, u32)) -> Vec<(i32, i32, i32, i32)> {
    let radius = RADIUS.min(width / 2).min(height / 2);
    let mut rows = Vec::new();
    for y in 0..radius {
        let dy = (radius - y) as f64 - 0.5;
        let inset = (radius as f64 - ((radius * radius) as f64 - dy * dy).sqrt()).ceil() as u32;
        let row = (inset as i32, y as i32, (width - inset * 2) as i32, 1);
        rows.push(row);
        rows.push((row.0, (height - y - 1) as i32, row.2, 1));
    }
    if height > radius * 2 {
        rows.push((0, radius as i32, width as i32, (height - radius * 2) as i32));
    }
    rows
}

impl Dispatch<ext_manager::ExtBackgroundEffectManagerV1, ()> for Worker {
    fn event(
        state: &mut Self,
        _: &ext_manager::ExtBackgroundEffectManagerV1,
        event: ext_manager::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ext_manager::Event::Capabilities { flags } = event {
            let mut material = state.material.0.lock().unwrap_or_else(|e| e.into_inner());
            material.capabilities =
                matches!(flags, WEnum::Value(value) if value.contains(ext_manager::Capability::Blur));
            material.refresh();
        }
    }
}

wayland_client::delegate_noop!(Worker: ignore wl_compositor::WlCompositor);
wayland_client::delegate_noop!(Worker: ignore wayland_client::protocol::wl_region::WlRegion);
wayland_client::delegate_noop!(Worker: ignore ext_surface::ExtBackgroundEffectSurfaceV1);
wayland_client::delegate_noop!(Worker: ignore kde_manager::OrgKdeKwinBlurManager);
wayland_client::delegate_noop!(Worker: ignore kde_surface::OrgKdeKwinBlur);

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::io::{Read, Write};
    use std::net::Shutdown;
    use std::os::unix::net::UnixStream;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::mpsc as channel;

    wayland_client::delegate_noop!(Worker: ignore wl_surface::WlSurface);

    struct DisconnectOnDrop(UnixStream);

    impl Drop for DisconnectOnDrop {
        fn drop(&mut self) {
            let _ = self.0.shutdown(Shutdown::Both);
        }
    }

    struct Peer {
        socket: Arc<Mutex<UnixStream>>,
        thread: Option<std::thread::JoinHandle<()>>,
        manager: Arc<AtomicU32>,
        registry: Arc<AtomicU32>,
        requests: channel::Receiver<(String, u16, Vec<u32>)>,
    }

    fn message(socket: &Mutex<UnixStream>, object: u32, opcode: u16, words: &[u32]) {
        let mut bytes = object.to_ne_bytes().to_vec();
        bytes.extend_from_slice(&(((words.len() as u32 * 4 + 8) << 16) | opcode as u32).to_ne_bytes());
        bytes.extend(words.iter().flat_map(|word| word.to_ne_bytes()));
        let _ = socket.lock().unwrap().write_all(&bytes);
    }

    impl Peer {
        fn new(standard: bool, legacy: bool) -> (Connection, Self) {
            let (client, mut reader) = UnixStream::pair().unwrap();
            let socket = Arc::new(Mutex::new(reader.try_clone().unwrap()));
            let writer = socket.clone();
            let manager = Arc::new(AtomicU32::new(0));
            let manager_id = manager.clone();
            let registry = Arc::new(AtomicU32::new(0));
            let registry_id = registry.clone();
            let (send, requests) = channel::channel();
            let thread = std::thread::spawn(move || {
                let _disconnect = DisconnectOnDrop(reader.try_clone().unwrap());
                let mut objects = HashMap::from([(1, "wl_display".to_string())]);
                loop {
                    let mut header = [0; 8];
                    if reader.read_exact(&mut header).is_err() {
                        break;
                    }
                    let object = u32::from_ne_bytes(header[..4].try_into().unwrap());
                    let packed = u32::from_ne_bytes(header[4..].try_into().unwrap());
                    let size = (packed >> 16) as usize;
                    assert!((8..=256).contains(&size));
                    let opcode = packed as u16;
                    let mut body = vec![0; size - 8];
                    if reader.read_exact(&mut body).is_err() {
                        break;
                    }
                    let words: Vec<_> = body.as_chunks::<4>().0.iter().map(|word| u32::from_ne_bytes(*word)).collect();
                    let interface = objects.get(&object).unwrap().clone();
                    match (interface.as_str(), opcode) {
                        ("wl_display", 1) => {
                            let id = words[0];
                            objects.insert(id, "wl_registry".into());
                            registry_id.store(id, Ordering::Release);
                            let globals = [
                                (1_u32, "wl_compositor", true),
                                (2, "ext_background_effect_manager_v1", standard),
                                (3, "org_kde_kwin_blur_manager", legacy),
                            ];
                            for (name, interface, advertise) in globals {
                                if !advertise {
                                    continue;
                                }
                                let mut payload = name.to_ne_bytes().to_vec();
                                payload.extend_from_slice(&((interface.len() + 1) as u32).to_ne_bytes());
                                payload.extend_from_slice(interface.as_bytes());
                                payload.push(0);
                                payload.resize(payload.len().next_multiple_of(4), 0);
                                payload.extend_from_slice(&1_u32.to_ne_bytes());
                                let words: Vec<_> =
                                    payload.as_chunks::<4>().0.iter().map(|word| u32::from_ne_bytes(*word)).collect();
                                message(&writer, id, 0, &words);
                            }
                        }
                        ("wl_display", 0) => {
                            message(&writer, words[0], 0, &[0]);
                            message(&writer, 1, 1, &[words[0]]);
                        }
                        ("wl_registry", 0) => {
                            let length = words[1] as usize;
                            let name = std::str::from_utf8(&body[8..8 + length - 1]).unwrap().to_string();
                            let id = *words.last().unwrap();
                            objects.insert(id, name.clone());
                            if name == "ext_background_effect_manager_v1" {
                                manager_id.store(id, Ordering::Release);
                                message(&writer, id, 0, &[1]);
                            }
                        }
                        ("wl_compositor", 0) => {
                            objects.insert(words[0], "wl_surface".into());
                        }
                        ("wl_compositor", 1) => {
                            objects.insert(words[0], "wl_region".into());
                        }
                        ("ext_background_effect_manager_v1", 1) => {
                            objects.insert(words[0], "ext_background_effect_surface_v1".into());
                        }
                        ("org_kde_kwin_blur_manager", 0) => {
                            objects.insert(words[0], "org_kde_kwin_blur".into());
                        }
                        _ => {}
                    }
                    let _ = send.send((interface, opcode, words));
                }
            });
            (
                Connection::from_socket(client).unwrap(),
                Self { socket, thread: Some(thread), manager, registry, requests },
            )
        }

        fn capabilities(&self, enabled: bool) {
            let manager = self.manager.load(Ordering::Acquire);
            assert_ne!(manager, 0);
            message(&self.socket, manager, 0, &[u32::from(enabled)]);
        }

        fn drain(&self) -> Vec<(String, u16, Vec<u32>)> {
            self.requests.try_iter().collect()
        }
    }

    impl Drop for Peer {
        fn drop(&mut self) {
            let _ = self.socket.lock().unwrap().shutdown(Shutdown::Both);
            let result = self.thread.take().unwrap().join();
            if !std::thread::panicking() {
                assert!(result.is_ok());
            }
        }
    }

    type Fixture = (
        Material,
        mpsc::UnboundedReceiver<bool>,
        wayland_client::EventQueue<Worker>,
        Worker,
        Peer,
        wl_surface::WlSurface,
    );

    fn setup(standard: bool, legacy: bool, allowed: bool) -> Fixture {
        let (connection, peer) = Peer::new(standard, legacy);
        let mut events = connection.new_event_queue();
        let queue = events.handle();
        let registry = connection.display().get_registry(&queue, ());
        let (material, updates) = Material::new(allowed, (720, 500));
        let mut worker = Worker::with_material(material.clone());
        events.roundtrip(&mut worker).unwrap();
        events.roundtrip(&mut worker).unwrap(); // bound manager's capabilities
        let compositor = registry.bind::<wl_compositor::WlCompositor, _, _>(1, 1, &queue, ());
        let surface = compositor.create_surface(&queue, ());
        material.surface(surface.clone(), &queue);
        events.roundtrip(&mut worker).unwrap();
        (material, updates, events, worker, peer, surface)
    }

    #[test]
    fn standard_blur_reuses_the_surface_and_honours_capability_withdrawal() {
        let (material, mut updates, mut events, mut worker, peer, surface) = setup(true, true, true);
        assert_eq!(updates.try_recv(), Ok(true));
        let requests = peer.drain();
        assert_eq!(
            requests.iter().filter(|(name, op, _)| name == "ext_background_effect_manager_v1" && *op == 1).count(),
            1
        );
        assert!(!requests.iter().any(|(name, op, _)| name == "org_kde_kwin_blur_manager" && *op == 0));
        assert!(!requests.iter().any(|(name, op, _)| name == "wl_surface" && *op == 6), "iced owns surface commits");
        assert!(requests.iter().any(|(name, op, args)| name == "wl_region" && *op == 1 && args == &[0, 14, 720, 472]));

        material.surface(surface, &events.handle());
        material.resize((980, 500));
        events.roundtrip(&mut worker).unwrap();
        let requests = peer.drain();
        assert!(!requests.iter().any(|(name, op, _)| name == "ext_background_effect_manager_v1" && *op == 1));
        assert!(requests.iter().any(|(name, op, args)| name == "wl_region" && *op == 1 && args == &[0, 14, 980, 472]));

        peer.capabilities(false);
        events.roundtrip(&mut worker).unwrap();
        assert_eq!(updates.try_recv(), Ok(false));
        events.roundtrip(&mut worker).unwrap();
        let requests = peer.drain();
        assert!(requests.iter().any(|(name, op, _)| name == "ext_background_effect_surface_v1" && *op == 0));
        assert!(!requests.iter().any(|(name, op, _)| name == "org_kde_kwin_blur_manager" && *op == 0));

        peer.capabilities(true);
        events.roundtrip(&mut worker).unwrap();
        assert_eq!(updates.try_recv(), Ok(true));
        events.roundtrip(&mut worker).unwrap();
        assert_eq!(
            peer.drain().iter().filter(|(name, op, _)| name == "ext_background_effect_manager_v1" && *op == 1).count(),
            1
        );

        message(&peer.socket, peer.registry.load(Ordering::Acquire), 1, &[2]);
        events.roundtrip(&mut worker).unwrap();
        events.roundtrip(&mut worker).unwrap();
        assert!(peer.drain().iter().any(|(name, op, _)| name == "org_kde_kwin_blur_manager" && *op == 0));
    }

    #[test]
    fn legacy_blur_and_missing_or_disabled_protocols_have_bounded_fallbacks() {
        for (legacy, allowed, expected) in [(true, true, true), (false, true, false), (true, false, false)] {
            let (material, mut updates, mut events, mut worker, peer, _) = setup(false, legacy, allowed);
            assert_eq!(updates.try_recv().ok(), expected.then_some(true));
            let requests = peer.drain();
            assert_eq!(requests.iter().any(|(name, op, _)| name == "org_kde_kwin_blur" && *op == 1), expected);
            if expected {
                material.resize((980, 500));
                events.roundtrip(&mut worker).unwrap();
                let resized = peer.drain();
                assert!(resized.iter().any(|(name, op, _)| name == "org_kde_kwin_blur" && *op == 2));
                assert!(resized.iter().any(|(name, op, _)| name == "org_kde_kwin_blur_manager" && *op == 0));
                assert!(
                    resized
                        .iter()
                        .any(|(name, op, args)| name == "wl_region" && *op == 1 && args == &[0, 14, 980, 472])
                );
                material.disconnect();
                events.roundtrip(&mut worker).unwrap();
                assert_eq!(updates.try_recv(), Ok(false));
                assert!(peer.drain().iter().any(|(name, op, _)| name == "org_kde_kwin_blur" && *op == 2));
            }
        }
    }
}
