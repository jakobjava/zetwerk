//! The clipboard and the primary selection of a Wayland compositor, on
//! a connection of zetwerk's own, through the compositor's data control
//! protocol (`wlr-data-control`, which sway, Hyprland, KDE and most
//! others offer; GNOME does not, and is served by `wl-paste` and
//! `wl-copy` instead).
//!
//! The window's own connection is winit's, which has no clipboard; the
//! ordinary one (`wl_data_device`) is also only for a window that has
//! the keyboard, at the moment it has it. Data control is what
//! `wl-clipboard` and clipboard managers use: it needs no window, tells
//! of every change of the selections as it happens (what kinds of data
//! are offered, not the data), and reads and writes them at any time.
//!
//! A thread of its own takes the compositor's events: what is copied
//! where, and requests for what zetwerk copied, which it writes out.
//! Reading is done on the thread that asks (never the window's: see
//! `viewer::paste`), bounded as the programs are.
//!
//! What zetwerk copied is held by this process: when it ends, it is
//! gone, unless a clipboard manager took it. [`Native::hand_over`] gives
//! it to `wl-copy` when the window closes, which stays to hold it.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::fd::AsFd;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use wayland_client::backend::ObjectId;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_registry, wl_seat::WlSeat};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, event_created_child};
use wayland_protocols_wlr::data_control::v1::client::zwlr_data_control_device_v1::{
    self, ZwlrDataControlDeviceV1,
};
use wayland_protocols_wlr::data_control::v1::client::zwlr_data_control_manager_v1::ZwlrDataControlManagerV1;
use wayland_protocols_wlr::data_control::v1::client::zwlr_data_control_offer_v1::{
    self, ZwlrDataControlOfferV1,
};
use wayland_protocols_wlr::data_control::v1::client::zwlr_data_control_source_v1::{
    self, ZwlrDataControlSourceV1,
};

use super::Selection;

/// The kinds of data zetwerk offers what it copies as: text, as every
/// program asks for it; and one that says it is zetwerk's.
const OFFERED: &[&str] = &[
    "text/plain;charset=utf-8",
    "text/plain",
    "UTF8_STRING",
    "STRING",
    "TEXT",
    OURS,
];

/// The kind that marks what zetwerk copied.
pub const OURS: &str = "application/x-zetwerk";

/// What a selection holds, as the compositor told of it.
struct Held {
    offer: ZwlrDataControlOfferV1,
    types: Vec<String>,
    /// Counts the changes of the selections: a new one, a new number.
    serial: u64,
}

/// What the thread of events and those that read and write share.
#[derive(Default)]
struct Shared {
    /// Offers told of and not (yet) made a selection: their kinds.
    offers: HashMap<ObjectId, (ZwlrDataControlOfferV1, Vec<String>)>,
    clipboard: Option<Held>,
    primary: Option<Held>,
    serial: u64,
    /// What zetwerk copied, by the source that offers it.
    sources: HashMap<ObjectId, Arc<Vec<u8>>>,
    /// The text zetwerk last copied into each selection, while it holds
    /// it: for `wl-copy` to take over when the window closes.
    own: [Option<(ObjectId, String)>; 2],
    /// The compositor took the data control away.
    gone: bool,
}

impl Shared {
    fn held(&mut self, selection: Selection) -> &mut Option<Held> {
        match selection {
            Selection::Clipboard => &mut self.clipboard,
            Selection::Primary => &mut self.primary,
        }
    }
}

/// The state the events are handed to.
struct State {
    shared: Arc<Mutex<Shared>>,
}

impl State {
    fn shared(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The selection is now `offer`, or nothing.
    fn selected(&self, selection: Selection, offer: Option<&ZwlrDataControlOfferV1>) {
        let mut shared = self.shared();
        shared.serial += 1;
        let serial = shared.serial;
        let held = offer.map(|offer| {
            let types = shared
                .offers
                .remove(&offer.id())
                .map(|(_, types)| types)
                .unwrap_or_default();
            Held {
                offer: offer.clone(),
                types,
                serial,
            }
        });
        // Offers that came before this one and went nowhere.
        for (_, (stale, _)) in shared.offers.drain() {
            stale.destroy();
        }
        if let Some(old) = std::mem::replace(shared.held(selection), held) {
            old.offer.destroy();
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn event(
        _: &mut Self,
        _: &WlSeat,
        _: <WlSeat as Proxy>::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrDataControlManagerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwlrDataControlManagerV1,
        _: <ZwlrDataControlManagerV1 as Proxy>::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrDataControlDeviceV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwlrDataControlDeviceV1,
        event: zwlr_data_control_device_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwlr_data_control_device_v1::Event;
        match event {
            Event::DataOffer { id } => {
                state.shared().offers.insert(id.id(), (id, Vec::new()));
            }
            Event::Selection { id } => state.selected(Selection::Clipboard, id.as_ref()),
            Event::PrimarySelection { id } => state.selected(Selection::Primary, id.as_ref()),
            Event::Finished => state.shared().gone = true,
            _ => {}
        }
    }

    event_created_child!(State, ZwlrDataControlDeviceV1, [
        zwlr_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (ZwlrDataControlOfferV1, ()),
    ]);
}

impl Dispatch<ZwlrDataControlOfferV1, ()> for State {
    fn event(
        state: &mut Self,
        offer: &ZwlrDataControlOfferV1,
        event: zwlr_data_control_offer_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_data_control_offer_v1::Event::Offer { mime_type } = event
            && let Some((_, types)) = state.shared().offers.get_mut(&offer.id())
        {
            types.push(mime_type);
        }
    }
}

impl Dispatch<ZwlrDataControlSourceV1, ()> for State {
    fn event(
        state: &mut Self,
        source: &ZwlrDataControlSourceV1,
        event: zwlr_data_control_source_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwlr_data_control_source_v1::Event;
        match event {
            Event::Send { fd, .. } => {
                let data = state.shared().sources.get(&source.id()).cloned();
                if let Some(data) = data {
                    // Written aside: the program that reads it may be
                    // slow, and other events wait for nobody.
                    let _ = std::thread::Builder::new()
                        .name("zetwerk-clipboard-send".to_owned())
                        .spawn(move || {
                            let mut file = std::fs::File::from(fd);
                            let _ = file.write_all(&data);
                        });
                }
            }
            Event::Cancelled => {
                let mut shared = state.shared();
                shared.sources.remove(&source.id());
                for own in &mut shared.own {
                    if own.as_ref().is_some_and(|(id, _)| *id == source.id()) {
                        *own = None;
                    }
                }
                source.destroy();
            }
            _ => {}
        }
    }
}

/// The clipboard of the compositor, through data control.
pub struct Native {
    connection: Connection,
    queue: QueueHandle<State>,
    manager: ZwlrDataControlManagerV1,
    device: ZwlrDataControlDeviceV1,
    /// Whether the compositor has the primary selection in it too.
    primary: bool,
    shared: Arc<Mutex<Shared>>,
}

/// The one there is, once it has been asked for: `None` where there is
/// no Wayland display, or the compositor has no data control.
static NATIVE: OnceLock<Option<Native>> = OnceLock::new();

/// The native clipboard, set up the first time it is asked for (a
/// connection, and one round to the compositor and back).
pub fn native() -> Option<&'static Native> {
    NATIVE.get_or_init(Native::connect).as_ref()
}

/// The native clipboard, if it has been set up; never waits for that.
pub fn native_now() -> Option<&'static Native> {
    NATIVE.get().and_then(Option::as_ref)
}

impl Native {
    fn connect() -> Option<Self> {
        let display = std::env::var_os("WAYLAND_DISPLAY").filter(|display| !display.is_empty());
        display?;
        let connection = Connection::connect_to_env().ok()?;
        let (globals, mut events) = registry_queue_init::<State>(&connection).ok()?;
        let queue = events.handle();
        let manager: ZwlrDataControlManagerV1 = globals.bind(&queue, 1..=2, ()).ok()?;
        let seat: WlSeat = globals.bind(&queue, 1..=1, ()).ok()?;
        let device = manager.get_data_device(&seat, &queue, ());
        let shared = Arc::new(Mutex::new(Shared::default()));
        let mut state = State {
            shared: Arc::clone(&shared),
        };
        // What the selections hold now.
        events.roundtrip(&mut state).ok()?;
        std::thread::Builder::new()
            .name("zetwerk-clipboard".to_owned())
            .spawn(move || {
                while events.blocking_dispatch(&mut state).is_ok() {
                    if state.shared().gone {
                        break;
                    }
                }
                state.shared().gone = true;
            })
            .ok()?;
        Some(Self {
            connection,
            queue,
            primary: manager.version() >= 2,
            manager,
            device,
            shared,
        })
    }

    fn shared(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether it still works.
    pub fn works(&self) -> bool {
        !self.shared().gone
    }

    /// What `selection` holds, as kinds of data, the number of its last
    /// change, and whether zetwerk copied it; `None` if it holds
    /// nothing, or the compositor has no such selection.
    pub fn offered(&self, selection: Selection) -> Option<(Vec<String>, u64, bool)> {
        let mut shared = self.shared();
        let held = shared.held(selection).as_ref()?;
        let ours = held.types.iter().any(|kind| kind == OURS);
        Some((held.types.clone(), held.serial, ours))
    }

    /// What `selection` holds as `kind`, read within `patience`, at
    /// most `most` bytes.
    pub fn read(
        &self,
        selection: Selection,
        kind: &str,
        patience: Duration,
        most: u64,
    ) -> Result<Vec<u8>, String> {
        let offer = self
            .shared()
            .held(selection)
            .as_ref()
            .map(|held| held.offer.clone())
            .ok_or_else(|| format!("the {} holds nothing", selection.name()))?;
        let (reader, writer) =
            std::io::pipe().map_err(|err| format!("the clipboard was not read: {err}"))?;
        offer.receive(kind.to_owned(), writer.as_fd());
        self.connection
            .flush()
            .map_err(|err| format!("the clipboard was not read: {err}"))?;
        drop(writer);
        // Read aside, waited for no longer than `patience`: what does
        // not answer is left to end when the program that copied does.
        let (done, result) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("zetwerk-clipboard-read".to_owned())
            .spawn(move || {
                let mut bytes = Vec::new();
                let read = reader.take(most).read_to_end(&mut bytes);
                let _ = done.send(read.map(|_| bytes));
            })
            .map_err(|err| format!("the clipboard was not read: {err}"))?;
        match result.recv_timeout(patience) {
            Ok(Ok(bytes)) => Ok(bytes),
            Ok(Err(err)) => Err(format!("the clipboard was not read: {err}")),
            Err(_) => Err(format!(
                "the clipboard was not read: the program that copied did not answer in {} s",
                patience.as_secs_f32()
            )),
        }
    }

    /// Puts `text` into `selection`, held by this process. Returns
    /// whether it could.
    pub fn write(&self, selection: Selection, text: &str) -> bool {
        if (selection == Selection::Primary && !self.primary) || !self.works() {
            return false;
        }
        let source = self.manager.create_data_source(&self.queue, ());
        for kind in OFFERED {
            source.offer((*kind).to_owned());
        }
        {
            let mut shared = self.shared();
            let bytes = Arc::new(text.as_bytes().to_vec());
            shared.sources.insert(source.id(), bytes);
            shared.own[selection as usize] = Some((source.id(), text.to_owned()));
        }
        match selection {
            Selection::Clipboard => self.device.set_selection(Some(&source)),
            Selection::Primary => self.device.set_primary_selection(Some(&source)),
        }
        self.connection.flush().is_ok()
    }

    /// What zetwerk copied into each selection and holds still: for
    /// another to hold once the window has closed.
    pub fn held_texts(&self) -> Vec<(Selection, String)> {
        let shared = self.shared();
        [Selection::Clipboard, Selection::Primary]
            .into_iter()
            .filter_map(|selection| {
                let (_, text) = shared.own[selection as usize].as_ref()?;
                Some((selection, text.clone()))
            })
            .collect()
    }
}
