// SPDX-License-Identifier: GPL-3.0-or-later
//! Read clipboard offers using the compositor's data-control protocol.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, PipeReader, Read};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd};
use std::os::unix::fs::FileExt;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use wayland_client::globals::Global;
use wayland_client::protocol::{wl_callback, wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, event_created_child};
use wayland_protocols::ext::data_control::v1::client::{
    ext_data_control_device_v1 as ext_device, ext_data_control_manager_v1 as ext_manager,
    ext_data_control_offer_v1 as ext_offer,
};
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_device_v1 as wlr_device, zwlr_data_control_manager_v1 as wlr_manager,
    zwlr_data_control_offer_v1 as wlr_offer,
};

use super::{Mode, Policy, Profile, TEXT_LIMIT, copy_time};

const MIME_TYPES: [&str; 4] = ["text/plain;charset=utf-8", "text/plain;charset=UTF-8", "UTF8_STRING", "text/plain"];
const SENSITIVE: &str = "x-kde-passwordManagerHint";
const WAIT_MS: i32 = 120;
const MAX_TRANSFERS: usize = 4;
const TRANSFER_DEADLINE: Duration = Duration::from_secs(1);
const SETUP_DEADLINE: Duration = Duration::from_secs(1);

#[derive(Default)]
struct Mime {
    rank: Option<usize>,
    sensitive: bool,
}

impl Mime {
    fn offer(&mut self, name: &str) {
        self.sensitive |= name.eq_ignore_ascii_case(SENSITIVE);
        if let Some(rank) = MIME_TYPES.iter().position(|mime| *mime == name) {
            self.rank = Some(self.rank.map_or(rank, |old| old.min(rank)));
        }
    }

    fn plain_text(&self) -> Option<&'static str> {
        self.rank.filter(|_| !self.sensitive).map(|rank| MIME_TYPES[rank])
    }
}

type Offer = Mutex<Mime>;

struct Transfer {
    pipe: PipeReader,
    policy: Policy,
    copied_at: Duration,
    deadline: Instant,
    bytes: Vec<u8>,
}

impl Transfer {
    fn read_step(&mut self) -> io::Result<Option<String>> {
        let mut buffer = [0; TEXT_LIMIT + 1];
        let count = self.pipe.read(&mut buffer)?;
        if count == 0 {
            return String::from_utf8(std::mem::take(&mut self.bytes)).map(Some).map_err(io::Error::other);
        }
        if self.bytes.len() + count > TEXT_LIMIT {
            return Err(io::Error::other("Clipboard entry exceeds the text limit."));
        }
        self.bytes.extend_from_slice(&buffer[..count]);
        Ok(None)
    }
}

struct Capture {
    policy: Policy,
    initial: bool,
    closed: bool,
    pending: VecDeque<Transfer>,
    last_copy: Duration,
}

impl Capture {
    fn new(policy: Policy) -> Self {
        Self { policy, initial: true, closed: false, pending: VecDeque::new(), last_copy: Duration::ZERO }
    }

    fn selection(&mut self, mime: Option<&'static str>, receive: impl FnOnce(String, BorrowedFd<'_>)) {
        // Ignore the selection already present at startup or after resuming.
        if std::mem::take(&mut self.initial) || self.policy.mode != Mode::Recording {
            return;
        }
        if let Some(mime) = mime
            && let Ok((pipe, send)) = io::pipe()
        {
            receive(mime.to_owned(), send.as_fd());
            if self.pending.len() == MAX_TRANSFERS {
                self.pending.pop_front();
            }
            self.last_copy = copy_time().max(self.last_copy + Duration::from_nanos(1));
            self.pending.push_back(Transfer {
                pipe,
                policy: self.policy,
                copied_at: self.last_copy,
                deadline: Instant::now() + TRANSFER_DEADLINE,
                bytes: Vec::new(),
            });
        }
    }

    fn drain(&mut self, profile: &Profile) -> io::Result<()> {
        for _ in 0..self.pending.len() {
            let Some(mut transfer) = self.pending.pop_front() else { break };
            loop {
                if !ready(transfer.pipe.as_fd(), 0)? {
                    // Expire waiting pipes without discarding data already ready to read.
                    if Instant::now() < transfer.deadline {
                        self.pending.push_back(transfer);
                    }
                    break;
                }
                match transfer.read_step() {
                    Ok(None) => {}
                    Ok(Some(text)) => {
                        profile.record(transfer.policy, text, transfer.copied_at)?;
                        break;
                    }
                    // A departing producer or oversized offer affects only that item.
                    Err(_) => break,
                }
            }
        }
        Ok(())
    }
}

enum Protocol {
    Standard,
    Wlr,
}

enum Device {
    Standard(ext_manager::ExtDataControlManagerV1, ext_device::ExtDataControlDeviceV1),
    Wlr(wlr_manager::ZwlrDataControlManagerV1, wlr_device::ZwlrDataControlDeviceV1),
}

type Globals = Arc<Mutex<Vec<Global>>>;
type SyncDone = Arc<AtomicBool>;

struct Registry {
    proxy: wl_registry::WlRegistry,
    globals: Vec<Global>,
}

impl Protocol {
    fn connect<D>(state: &mut D, deadline: Instant) -> io::Result<(Connection, Registry, EventQueue<D>, Self)>
    where
        D: Dispatch<wl_registry::WlRegistry, Globals> + Dispatch<wl_callback::WlCallback, SyncDone> + 'static,
    {
        let connection = Connection::connect_to_env().map_err(io::Error::other)?;
        let mut queue = connection.new_event_queue();
        let globals = Globals::default();
        let proxy = connection.display().get_registry(&queue.handle(), Arc::clone(&globals));
        roundtrip_until(&connection, &mut queue, state, deadline)?;
        let globals =
            std::mem::take(&mut *globals.lock().map_err(|_| io::Error::other("Cannot read Wayland globals."))?);
        let supports = |interface| globals.iter().any(|global| global.interface == interface);
        let protocol = if supports("ext_data_control_manager_v1") {
            Self::Standard
        } else if supports("zwlr_data_control_manager_v1") {
            Self::Wlr
        } else {
            return Err(io::Error::other("This Wayland compositor does not offer clipboard data control."));
        };
        Ok((connection, Registry { proxy, globals }, queue, protocol))
    }

    fn bind<D>(self, registry: &Registry, handle: &QueueHandle<D>) -> io::Result<Device>
    where
        D: Dispatch<wl_seat::WlSeat, ()>
            + Dispatch<ext_manager::ExtDataControlManagerV1, ()>
            + Dispatch<ext_device::ExtDataControlDeviceV1, ()>
            + Dispatch<wlr_manager::ZwlrDataControlManagerV1, ()>
            + Dispatch<wlr_device::ZwlrDataControlDeviceV1, ()>
            + 'static,
    {
        let seat: wl_seat::WlSeat = registry.bind(handle, 1)?;
        match self {
            Self::Standard => {
                let manager: ext_manager::ExtDataControlManagerV1 = registry.bind(handle, 1)?;
                let device = manager.get_data_device(&seat, handle, ());
                Ok(Device::Standard(manager, device))
            }
            Self::Wlr => {
                let manager: wlr_manager::ZwlrDataControlManagerV1 = registry.bind(handle, 2)?;
                let device = manager.get_data_device(&seat, handle, ());
                Ok(Device::Wlr(manager, device))
            }
        }
    }
}

impl Registry {
    fn bind<P, D>(&self, queue: &QueueHandle<D>, version: u32) -> io::Result<P>
    where
        P: Proxy + 'static,
        D: Dispatch<P, ()> + 'static,
    {
        let interface = P::interface().name;
        let global = self
            .globals
            .iter()
            .find(|global| global.interface == interface)
            .ok_or_else(|| io::Error::other("The Wayland seat or clipboard protocol is unavailable."))?;
        Ok(self.proxy.bind(global.name, global.version.min(version), queue, ()))
    }
}

pub(super) fn probe() -> io::Result<()> {
    let deadline = Instant::now() + SETUP_DEADLINE;
    let mut state = Capture::new(Policy::default());
    let (connection, globals, mut queue, protocol) = Protocol::connect(&mut state, deadline)?;
    protocol.bind(&globals, &queue.handle())?;
    roundtrip_until(&connection, &mut queue, &mut state, deadline)?;
    if state.closed {
        return Err(io::Error::other("This Wayland seat does not allow clipboard recording."));
    }
    Ok(())
}

pub(super) fn watch(profile: &Profile, lock: &File, policy: Policy) -> io::Result<()> {
    if policy.mode != Mode::Recording {
        return Ok(());
    }
    let deadline = Instant::now() + SETUP_DEADLINE;
    let mut state = Capture::new(policy);
    let (connection, globals, mut queue, protocol) = Protocol::connect(&mut state, deadline)?;
    protocol.bind(&globals, &queue.handle())?;
    roundtrip_until(&connection, &mut queue, &mut state, deadline)?;
    if state.closed {
        return Ok(());
    }
    lock.write_all_at(&policy.revision.to_le_bytes(), 0)?;
    let mut maintenance = Instant::now() + Duration::from_secs(60);
    loop {
        if state.closed {
            return Ok(());
        }
        state.policy = profile.policy()?;
        if state.policy.mode != Mode::Recording || state.policy.revision != policy.revision {
            return Ok(());
        }
        advance(&mut queue, &mut state, WAIT_MS)?;
        state.drain(profile)?;
        if Instant::now() >= maintenance {
            profile.snapshot()?;
            maintenance = Instant::now() + Duration::from_secs(60);
        }
    }
}

fn roundtrip_until<D>(
    connection: &Connection,
    queue: &mut EventQueue<D>,
    state: &mut D,
    deadline: Instant,
) -> io::Result<()>
where
    D: Dispatch<wl_callback::WlCallback, SyncDone> + 'static,
{
    let done = SyncDone::default();
    connection.display().sync(&queue.handle(), Arc::clone(&done));
    while !done.load(Ordering::Relaxed) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "The desktop did not respond to clipboard setup."));
        }
        advance(queue, state, remaining.as_millis().clamp(1, i32::MAX as u128) as i32)?;
    }
    Ok(())
}

fn advance<D: 'static>(queue: &mut EventQueue<D>, state: &mut D, timeout: i32) -> io::Result<()> {
    let dispatched = queue.dispatch_pending(state).map_err(io::Error::other)?;
    queue.flush().map_err(io::Error::other)?;
    if dispatched != 0 {
        return Ok(());
    }
    let Some(read) = queue.prepare_read() else { return Ok(()) };
    if ready(read.connection_fd(), timeout)? {
        read.read().map_err(io::Error::other)?;
    }
    Ok(())
}

fn ready(fd: BorrowedFd<'_>, timeout: i32) -> io::Result<bool> {
    let mut descriptor = libc::pollfd { fd: fd.as_raw_fd(), events: libc::POLLIN, revents: 0 };
    // poll borrows the live descriptors for the duration of the call.
    let result = unsafe { libc::poll(&mut descriptor, 1, timeout) };
    if result >= 0 {
        return Ok(result > 0);
    }
    let error = io::Error::last_os_error();
    if error.kind() == io::ErrorKind::Interrupted { Ok(false) } else { Err(error) }
}

// These objects have no events the recorder needs to handle.
macro_rules! ignore_events {
    ($state:ty, $object:ty, $data:ty) => {
        impl Dispatch<$object, $data> for $state {
            fn event(
                _: &mut Self,
                _: &$object,
                _: <$object as wayland_client::Proxy>::Event,
                _: &$data,
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
            }
        }
    };
}

macro_rules! dispatch_setup {
    ($state:ty) => {
        impl Dispatch<wl_registry::WlRegistry, Globals> for $state {
            fn event(
                _: &mut Self,
                _: &wl_registry::WlRegistry,
                event: wl_registry::Event,
                globals: &Globals,
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
                if let Ok(mut globals) = globals.lock() {
                    match event {
                        wl_registry::Event::Global { name, interface, version } => {
                            globals.push(wayland_client::globals::Global { name, interface, version });
                        }
                        wl_registry::Event::GlobalRemove { name } => globals.retain(|global| global.name != name),
                        _ => {}
                    }
                }
            }
        }

        impl Dispatch<wl_callback::WlCallback, SyncDone> for $state {
            fn event(
                _: &mut Self,
                _: &wl_callback::WlCallback,
                _: wl_callback::Event,
                done: &SyncDone,
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
                done.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
    };
}

pub mod publish;

dispatch_setup!(Capture);
ignore_events!(Capture, wl_seat::WlSeat, ());

// Both data-control versions expose the same offer and selection semantics.
macro_rules! capture_protocol {
    ($manager:ident::$Manager:ident, $device:ident::$Device:ident, $offer:ident::$Offer:ident) => {
        ignore_events!(Capture, $manager::$Manager, ());

        impl Dispatch<$device::$Device, ()> for Capture {
            fn event(
                state: &mut Self,
                _: &$device::$Device,
                event: $device::Event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
                match event {
                    $device::Event::Selection { id } => {
                        let mime = id.as_ref()
                            .and_then(Proxy::data::<Offer>)
                            .and_then(|offer| offer.lock().ok())
                            .and_then(|offer| offer.plain_text());
                        state.selection(mime, |mime, fd| {
                            if let Some(offer) = &id { offer.receive(mime, fd); }
                        });
                        if let Some(offer) = id { offer.destroy(); }
                    }
                    $device::Event::PrimarySelection { id: Some(offer) } => offer.destroy(),
                    $device::Event::Finished => state.closed = true,
                    _ => {}
                }
            }

            event_created_child!(Capture, $device::$Device, [
                0 => ($offer::$Offer, Offer::default()),
            ]);
        }

        impl Dispatch<$offer::$Offer, Offer> for Capture {
            fn event(
                _: &mut Self,
                _: &$offer::$Offer,
                event: $offer::Event,
                offer: &Offer,
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
                if let $offer::Event::Offer { mime_type } = event
                    && let Ok(mut offer) = offer.lock()
                {
                    offer.offer(&mime_type);
                }
            }
        }
    };
}

capture_protocol!(
    ext_manager::ExtDataControlManagerV1,
    ext_device::ExtDataControlDeviceV1,
    ext_offer::ExtDataControlOfferV1
);
capture_protocol!(
    wlr_manager::ZwlrDataControlManagerV1,
    wlr_device::ZwlrDataControlDeviceV1,
    wlr_offer::ZwlrDataControlOfferV1
);

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::net::UnixStream;

    #[test]
    fn setup_times_out_when_the_compositor_does_not_answer() {
        let (client, _silent_peer) = UnixStream::pair().unwrap();
        let connection = Connection::from_socket(client).unwrap();
        let mut queue = connection.new_event_queue();
        let mut capture = Capture::new(Policy::default());
        let deadline = Instant::now() + Duration::from_millis(25);
        let error = roundtrip_until(&connection, &mut queue, &mut capture, deadline).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }

    #[test]
    fn pending_copies_are_recorded_without_waiting_for_an_older_producer() {
        let dir = tempfile::tempdir().unwrap();
        let profile = Profile(dir.path().join("history"));
        let policy = Policy { mode: Mode::Recording, ..Policy::default() };
        super::super::write_atomic(&profile.policy_path(), &policy).unwrap();
        let mut capture = Capture::new(policy);
        capture.initial = false;

        let mut older = None;
        capture.selection(Some("text/plain"), |_, fd| {
            older = Some(File::from(fd.try_clone_to_owned().unwrap()));
        });
        capture.selection(Some("text/plain"), |_, fd| {
            File::from(fd.try_clone_to_owned().unwrap()).write_all(b"Newer").unwrap();
        });

        capture.drain(&profile).unwrap();
        let entries = profile.snapshot().unwrap().entries;
        assert_eq!(entries.iter().map(|entry| entry.text.as_str()).collect::<Vec<_>>(), ["Newer"]);
        older.unwrap().write_all(b"Older").unwrap();
        capture.drain(&profile).unwrap();
        let entries = profile.snapshot().unwrap().entries;
        assert_eq!(entries.iter().map(|entry| entry.text.as_str()).collect::<Vec<_>>(), ["Newer", "Older"]);
    }

    #[test]
    fn completed_copies_are_recorded_even_after_the_waiting_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let profile = Profile(dir.path().join("history"));
        let policy = Policy { mode: Mode::Recording, ..Policy::default() };
        super::super::write_atomic(&profile.policy_path(), &policy).unwrap();
        let mut capture = Capture::new(policy);
        capture.initial = false;

        let mut held_open = None;
        capture.selection(Some("text/plain"), |_, fd| held_open = Some(fd.try_clone_to_owned().unwrap()));
        capture.selection(Some("text/plain"), |_, fd| {
            File::from(fd.try_clone_to_owned().unwrap()).write_all(b"Ready").unwrap();
        });
        for transfer in &mut capture.pending {
            transfer.deadline = Instant::now();
        }

        capture.drain(&profile).unwrap();
        let entries = profile.snapshot().unwrap().entries;
        assert_eq!(entries.iter().map(|entry| entry.text.as_str()).collect::<Vec<_>>(), ["Ready"]);
        assert!(capture.pending.is_empty());
        drop(held_open);
    }

    #[test]
    fn sensitive_metadata_excludes_a_plain_text_offer() {
        let mut offer = Mime::default();
        offer.offer("image/png");
        assert_eq!(offer.plain_text(), None);
        offer.offer("text/plain");
        offer.offer("text/plain;charset=utf-8");
        assert_eq!(offer.plain_text(), Some("text/plain;charset=utf-8"));
        offer.offer(SENSITIVE);
        assert_eq!(offer.plain_text(), None);
    }
}
