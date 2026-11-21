// SPDX-License-Identifier: GPL-3.0-or-later
//! Read clipboard offers using the compositor's data-control protocol.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, PipeReader, Read};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd};
use std::os::unix::fs::FileExt;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use wayland_client::globals::{GlobalList, GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, event_created_child};
use wayland_protocols::ext::data_control::v1::client::{
    ext_data_control_device_v1 as ext_device, ext_data_control_manager_v1 as ext_manager,
    ext_data_control_offer_v1 as ext_offer,
};
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_device_v1 as wlr_device, zwlr_data_control_manager_v1 as wlr_manager,
    zwlr_data_control_offer_v1 as wlr_offer,
};

use super::{Mode, Policy, Profile, TEXT_LIMIT};

const MIME_TYPES: [&str; 4] = ["text/plain;charset=utf-8", "text/plain;charset=UTF-8", "UTF8_STRING", "text/plain"];
const SENSITIVE: &str = "x-kde-passwordManagerHint";
const WAIT_MS: i32 = 120;
const MAX_TRANSFERS: usize = 4;
const TRANSFER_DEADLINE: Duration = Duration::from_secs(1);

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
}

impl Capture {
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
            self.pending.push_back(Transfer {
                pipe,
                policy: self.policy,
                deadline: Instant::now() + TRANSFER_DEADLINE,
                bytes: Vec::new(),
            });
        }
    }

    fn drain(&mut self, profile: &Profile) -> io::Result<()> {
        while let Some(transfer) = self.pending.front_mut() {
            if Instant::now() >= transfer.deadline {
                self.pending.pop_front();
                continue;
            }
            if !ready(transfer.pipe.as_fd(), 0)? {
                break;
            }
            match transfer.read_step() {
                Ok(Some(text)) => {
                    profile.record(transfer.policy, text)?;
                    self.pending.pop_front();
                }
                Ok(None) => {}
                // A departing producer or oversized offer affects only that item.
                Err(_) => {
                    self.pending.pop_front();
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

impl Protocol {
    fn connect() -> io::Result<(GlobalList, EventQueue<Capture>, Self)> {
        let connection = Connection::connect_to_env().map_err(io::Error::other)?;
        let (globals, queue) = registry_queue_init(&connection).map_err(io::Error::other)?;
        let listed = globals.contents().clone_list();
        let supports = |interface| listed.iter().any(|global| global.interface == interface);
        let protocol = if supports("ext_data_control_manager_v1") {
            Self::Standard
        } else if supports("zwlr_data_control_manager_v1") {
            Self::Wlr
        } else {
            return Err(io::Error::other("This Wayland compositor does not offer clipboard data control."));
        };
        Ok((globals, queue, protocol))
    }

    fn bind(self, globals: &GlobalList, handle: &QueueHandle<Capture>) -> io::Result<()> {
        let seat: wl_seat::WlSeat = globals.bind(handle, 1..=1, ()).map_err(io::Error::other)?;
        match self {
            Self::Standard => {
                let manager: ext_manager::ExtDataControlManagerV1 =
                    globals.bind(handle, 1..=1, ()).map_err(io::Error::other)?;
                manager.get_data_device(&seat, handle, ());
            }
            Self::Wlr => {
                let manager: wlr_manager::ZwlrDataControlManagerV1 =
                    globals.bind(handle, 1..=2, ()).map_err(io::Error::other)?;
                manager.get_data_device(&seat, handle, ());
            }
        }
        Ok(())
    }
}

pub(super) fn probe() -> io::Result<()> {
    let (globals, mut queue, protocol) = Protocol::connect()?;
    protocol.bind(&globals, &queue.handle())?;
    let mut state = Capture { policy: Policy::default(), initial: true, closed: false, pending: VecDeque::new() };
    queue.roundtrip(&mut state).map_err(io::Error::other)?;
    if state.closed {
        return Err(io::Error::other("This Wayland seat does not allow clipboard recording."));
    }
    Ok(())
}

/// Returns true to rebind after a new recording session starts.
pub(super) fn watch(profile: &Profile, lock: &File) -> io::Result<bool> {
    let policy = profile.policy()?;
    if policy.mode != Mode::Recording {
        return Ok(false);
    }
    let (globals, mut queue, protocol) = Protocol::connect()?;
    protocol.bind(&globals, &queue.handle())?;
    let mut state = Capture { policy, initial: true, closed: false, pending: VecDeque::new() };
    queue.roundtrip(&mut state).map_err(io::Error::other)?;
    if state.closed {
        return Ok(false);
    }
    lock.write_all_at(&policy.revision.to_le_bytes(), 0)?;
    let mut maintenance = Instant::now() + Duration::from_secs(60);
    loop {
        state.policy = profile.policy()?;
        if state.closed || state.policy.mode != Mode::Recording {
            return Ok(false);
        }
        if state.policy.revision != policy.revision {
            return Ok(true);
        }
        advance(&mut queue, &mut state)?;
        state.drain(profile)?;
        if Instant::now() >= maintenance {
            profile.snapshot()?;
            maintenance = Instant::now() + Duration::from_secs(60);
        }
    }
}

fn advance(queue: &mut EventQueue<Capture>, state: &mut Capture) -> io::Result<()> {
    queue.dispatch_pending(state).map_err(io::Error::other)?;
    queue.flush().map_err(io::Error::other)?;
    if let Some(read) = queue.prepare_read()
        && ready(read.connection_fd(), WAIT_MS)?
    {
        read.read().map_err(io::Error::other)?;
    }
    Ok(())
}

fn ready(fd: BorrowedFd<'_>, timeout: i32) -> io::Result<bool> {
    let mut fd = libc::pollfd { fd: fd.as_raw_fd(), events: libc::POLLIN, revents: 0 };
    // poll borrows one live descriptor for the duration of this call.
    let result = unsafe { libc::poll(&mut fd, 1, timeout) };
    if result >= 0 {
        return Ok(result > 0);
    }
    let error = io::Error::last_os_error();
    if error.kind() == io::ErrorKind::Interrupted { Ok(false) } else { Err(error) }
}

// These objects have no events the recorder needs to handle.
macro_rules! ignore_events {
    ($object:ty, $data:ty) => {
        impl Dispatch<$object, $data> for Capture {
            fn event(
                _: &mut Self,
                _: &$object,
                _: <$object as Proxy>::Event,
                _: &$data,
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
            }
        }
    };
}

ignore_events!(wl_registry::WlRegistry, GlobalListContents);
ignore_events!(wl_seat::WlSeat, ());

// Both data-control versions expose the same offer and selection semantics.
macro_rules! capture_protocol {
    ($manager:ident::$Manager:ident, $device:ident::$Device:ident, $offer:ident::$Offer:ident) => {
        ignore_events!($manager::$Manager, ());

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
