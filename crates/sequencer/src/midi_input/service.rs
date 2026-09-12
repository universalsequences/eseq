//! Live device discovery and preferences. Only the worker touches MIDI drivers
//! or disk; callbacks and lifecycle changes share one ordered event stream.
use super::{parse_message, MidiInputEvent, MidiMessage, MidiNoteEvent, WakeFn, MAX_INPUT_PORTS};
use midir::{Ignore, MidiInput, MidiInputConnection, MidiInputPort};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub connected: bool,
    pub status: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub devices: Vec<Device>,
    pub error: String,
}

pub enum Event {
    Message(MidiInputEvent),
    Snapshot(Snapshot),
    /// Lifecycle cleanup cannot be consumed by user controller mappings.
    ResetPort(usize),
}

pub enum Command {
    Refresh,
    SetEnabled { id: String, enabled: bool },
    Stop,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct Preferences {
    // Retain disabled devices while offline. IDs are backend identities, never
    // enumeration indices or display names (two keyboards may have one name).
    disabled: BTreeMap<String, String>,
}

impl Preferences {
    fn load(path: &Path) -> Result<Self, String> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| e.to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.to_string()),
        }
    }

    fn save(&self, path: &Path) -> Result<(), String> {
        let parent = path
            .parent()
            .ok_or("MIDI preferences have no parent directory")?;
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        file.write_all(&bytes).map_err(|e| e.to_string())?;
        file.as_file().sync_all().map_err(|e| e.to_string())?;
        file.persist(path).map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[derive(Clone)]
struct Sink {
    tx: mpsc::Sender<Event>,
    wake: Option<WakeFn>,
}

impl Sink {
    fn send(&self, event: Event) {
        if self.tx.send(event).is_ok() {
            if let Some(wake) = &self.wake {
                wake();
            }
        }
    }
}

struct Ingress {
    active: bool,
    held: [[bool; 128]; 16],
    port: usize,
    sink: Sink,
}

impl Ingress {
    fn message(&mut self, message: MidiMessage) {
        if !self.active {
            return;
        }
        if let MidiMessage::Note { channel, note } = message {
            self.held[channel as usize][note.note as usize] = note.on;
        }
        self.sink.send(Event::Message(MidiInputEvent {
            port: self.port,
            message,
        }));
    }

    fn close(&mut self) {
        // Serialize against callbacks before retiring/reusing the slot. Late
        // callbacks are rejected, and all releases precede its next note-on.
        if !self.active {
            return;
        }
        for channel in 0..16 {
            for note in 0..128 {
                if self.held[channel][note] {
                    self.message(MidiMessage::Note {
                        channel: channel as u8,
                        note: MidiNoteEvent {
                            note: note as u8,
                            velocity: 0.0,
                            on: false,
                        },
                    });
                }
            }
        }
        self.active = false;
        self.sink.send(Event::ResetPort(self.port));
    }
}

struct Connection<C, P> {
    port: P,
    id: String,
    ingress: Arc<Mutex<Ingress>>,
    _driver: C,
}

impl<C, P> Drop for Connection<C, P> {
    fn drop(&mut self) {
        self.ingress.lock().unwrap().close();
    }
}

trait Driver {
    type Port;
    type Connection;
    fn ports(&mut self) -> Result<Vec<(String, String, Self::Port)>, String>;
    fn valid(&mut self, port: &Self::Port) -> bool;
    fn connect(
        &mut self,
        port: &Self::Port,
        ingress: Arc<Mutex<Ingress>>,
    ) -> Result<Self::Connection, String>;
}

#[derive(Default)]
struct SystemDriver {
    probe: Option<MidiInput>,
}

impl Driver for SystemDriver {
    type Port = MidiInputPort;
    type Connection = MidiInputConnection<()>;

    fn ports(&mut self) -> Result<Vec<(String, String, Self::Port)>, String> {
        if self.probe.is_none() {
            self.probe = Some(MidiInput::new("eseq-midi-discovery").map_err(|e| e.to_string())?);
        }
        let probe = self.probe.as_ref().unwrap();
        probe
            .ports()
            .into_iter()
            .map(|port| {
                let name = probe.port_name(&port).map_err(|e| e.to_string())?;
                Ok((port.id(), name, port))
            })
            .collect()
    }

    fn valid(&mut self, port: &MidiInputPort) -> bool {
        self.probe
            .as_ref()
            .is_some_and(|probe| probe.port_name(port).is_ok())
    }

    fn connect(
        &mut self,
        port: &MidiInputPort,
        ingress: Arc<Mutex<Ingress>>,
    ) -> Result<Self::Connection, String> {
        let mut input = MidiInput::new("eseq-midi-input").map_err(|e| e.to_string())?;
        input.ignore(Ignore::All);
        input
            .connect(
                port,
                "eseq-midi-input",
                move |_, bytes, _| {
                    if let Some(message) = parse_message(bytes) {
                        ingress.lock().unwrap().message(message);
                    }
                },
                (),
            )
            .map_err(|e| e.to_string())
    }
}

struct Manager<D: Driver> {
    driver: D,
    connections: Vec<Option<Connection<D::Connection, D::Port>>>,
    prefs: Preferences,
    persistent: bool,
    prefs_error: String,
    snapshot: Snapshot,
    sink: Sink,
}

impl<D: Driver> Manager<D> {
    fn new(driver: D, sink: Sink, path: &Path, persistent: bool) -> Self {
        let (prefs, prefs_error) = match if persistent {
            Preferences::load(path)
        } else {
            Ok(Preferences::default())
        } {
            Ok(prefs) => (prefs, String::new()),
            Err(e) => (
                Preferences::default(),
                format!("Could not read MIDI settings: {e}"),
            ),
        };
        Self {
            driver,
            connections: (0..MAX_INPUT_PORTS).map(|_| None).collect(),
            prefs,
            persistent,
            prefs_error,
            snapshot: Snapshot::default(),
            sink,
        }
    }

    fn scan(&mut self) {
        let ports = match self.driver.ports() {
            Ok(ports) => ports,
            Err(e) => {
                // A failed enumeration is not evidence that all devices left.
                self.snapshot.error = format!("Could not scan MIDI inputs: {e}");
                return;
            }
        };
        // ALSA IDs are client:port addresses, not persistent hardware IDs.
        // Forget a session selection when that address leaves the system.
        if !self.persistent {
            self.prefs
                .disabled
                .retain(|id, _| ports.iter().any(|(present, _, _)| present == id));
        }
        for slot in &mut self.connections {
            if slot.as_ref().is_some_and(|c| {
                self.prefs.disabled.contains_key(&c.id)
                || !ports.iter().any(|(id, _, _)| id == &c.id)
                // The endpoint may have been replaced between scans, keeping
                // its persistent ID while invalidating the old driver handle.
                || !self.driver.valid(&c.port)
            }) {
                *slot = None;
            }
        }
        let mut devices = Vec::new();
        for (id, name, port) in ports {
            let enabled = !self.prefs.disabled.contains_key(&id);
            let mut connected = self.connections.iter().flatten().any(|c| c.id == id);
            let mut status = if enabled { "Connected" } else { "Disabled" }.to_string();
            if enabled && !connected {
                if let Some(slot) = self.connections.iter().position(Option::is_none) {
                    let ingress = Arc::new(Mutex::new(Ingress {
                        active: true,
                        held: [[false; 128]; 16],
                        port: slot,
                        sink: self.sink.clone(),
                    }));
                    match self.driver.connect(&port, ingress.clone()) {
                        Ok(driver) => {
                            self.connections[slot] = Some(Connection {
                                id: id.clone(),
                                port,
                                ingress,
                                _driver: driver,
                            });
                            connected = true;
                        }
                        Err(e) => {
                            ingress.lock().unwrap().close();
                            status = format!("Could not connect: {e}");
                        }
                    }
                } else {
                    status = format!("Input limit reached ({MAX_INPUT_PORTS})");
                }
            }
            devices.push(Device {
                id,
                name,
                enabled,
                connected,
                status,
            });
        }
        for (id, name) in &self.prefs.disabled {
            if !devices.iter().any(|d| &d.id == id) {
                devices.push(Device {
                    id: id.clone(),
                    name: name.clone(),
                    enabled: false,
                    connected: false,
                    status: "Offline · disabled".into(),
                });
            }
        }
        self.snapshot = Snapshot {
            devices,
            error: self.prefs_error.clone(),
        };
    }

    fn set_enabled(&mut self, id: &str, enabled: bool, path: &Path) {
        let Some(device) = self.snapshot.devices.iter().find(|d| d.id == id) else {
            return;
        };
        let mut prefs = self.prefs.clone();
        if enabled {
            prefs.disabled.remove(id);
        } else {
            prefs.disabled.insert(id.to_string(), device.name.clone());
        }
        match if self.persistent {
            prefs.save(path)
        } else {
            Ok(())
        } {
            Ok(()) => {
                self.prefs = prefs;
                self.prefs_error.clear();
            }
            Err(e) => self.prefs_error = format!("Could not save MIDI settings: {e}"),
        }
    }
}

/// midir's ALSA backend exposes client:port addresses, which can be assigned
/// to unrelated devices on a later launch. Never persist those addresses.
pub fn persistent_device_ids() -> bool {
    !cfg!(target_os = "linux")
}

pub struct Service {
    pub commands: mpsc::Sender<Command>,
    rx: mpsc::Receiver<Event>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Service {
    pub fn start(wake: Option<WakeFn>) -> Result<Self, std::io::Error> {
        let path: PathBuf = crate::app_paths::app_paths()
            .preferences_path()
            .with_file_name("midi-input.json");
        Self::start_at(wake, path)
    }

    fn start_at(wake: Option<WakeFn>, path: PathBuf) -> Result<Self, std::io::Error> {
        let (commands, command_rx) = mpsc::channel();
        let (tx, rx) = mpsc::channel();
        let sink = Sink { tx, wake };
        let worker = std::thread::Builder::new()
            .name("midi-devices".into())
            .spawn(move || {
                let mut manager = Manager::new(
                    SystemDriver::default(),
                    sink.clone(),
                    &path,
                    persistent_device_ids(),
                );
                let mut previous = None;
                loop {
                    manager.scan();
                    if previous.as_ref() != Some(&manager.snapshot) {
                        sink.send(Event::Snapshot(manager.snapshot.clone()));
                        previous = Some(manager.snapshot.clone());
                    }
                    match command_rx.recv_timeout(Duration::from_secs(1)) {
                        Ok(Command::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Ok(Command::SetEnabled { id, enabled }) => {
                            manager.set_enabled(&id, enabled, &path)
                        }
                        Ok(Command::Refresh) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                }
            })?;
        Ok(Self {
            commands,
            rx,
            worker: Some(worker),
        })
    }

    pub fn drain(&self) -> impl Iterator<Item = Event> + '_ {
        self.rx.try_iter()
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Stop);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakeDriver {
        ports: Vec<String>,
        opened: Vec<(String, Arc<Mutex<Ingress>>)>,
        invalid_once: Option<String>,
        fail_scan: bool,
        fail_connect: bool,
    }
    impl Driver for FakeDriver {
        type Port = String;
        type Connection = ();
        fn ports(&mut self) -> Result<Vec<(String, String, String)>, String> {
            if self.fail_scan {
                return Err("scan failed".into());
            }
            // Deliberately give every device the same name.
            Ok(self
                .ports
                .iter()
                .map(|id| (id.clone(), "Keyboard".into(), id.clone()))
                .collect())
        }
        fn valid(&mut self, port: &String) -> bool {
            if self.invalid_once.as_ref() == Some(port) {
                self.invalid_once.take();
                false
            } else {
                true
            }
        }
        fn connect(&mut self, port: &String, ingress: Arc<Mutex<Ingress>>) -> Result<(), String> {
            if self.fail_connect {
                return Err("busy".into());
            }
            self.opened.push((port.clone(), ingress));
            Ok(())
        }
    }
    fn harness() -> (
        tempfile::TempDir,
        Manager<FakeDriver>,
        mpsc::Receiver<Event>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel();
        let manager = Manager::new(
            FakeDriver::default(),
            Sink { tx, wake: None },
            &dir.path().join("prefs.json"),
            true,
        );
        (dir, manager, rx)
    }
    fn note(on: bool) -> MidiMessage {
        MidiMessage::Note {
            channel: 3,
            note: MidiNoteEvent {
                note: 60,
                velocity: if on { 0.8 } else { 0.0 },
                on,
            },
        }
    }

    #[test]
    fn hotplug_preserves_connections_and_releases_before_slot_reuse() {
        let (_dir, mut m, rx) = harness();
        m.scan();
        assert!(m.snapshot.devices.is_empty());
        m.driver.ports = vec!["a".into(), "b".into()];
        m.scan();
        let old = m.driver.opened[0].1.clone();
        old.lock().unwrap().message(note(true));
        m.driver.ports.reverse();
        m.scan();
        assert_eq!(
            m.driver.opened.len(),
            2,
            "rescan/reorder must not reopen inputs"
        );
        assert_eq!(m.connections[0].as_ref().unwrap().id, "a");
        m.driver.ports = vec!["b".into(), "c".into()];
        m.scan();
        old.lock().unwrap().message(note(true)); // stale callback after close
        m.driver.opened[2].1.lock().unwrap().message(note(true));
        let events: Vec<_> = rx.try_iter().collect();
        assert_eq!(events.len(), 4);
        assert!(matches!(events[0], Event::Message(e) if e.port == 0 && e.message == note(true)));
        assert!(matches!(events[1], Event::Message(e) if e.port == 0 && e.message == note(false)));
        assert!(matches!(events[2], Event::ResetPort(0)));
        assert!(matches!(events[3], Event::Message(e) if e.port == 0 && e.message == note(true)));
        assert_eq!(m.connections[1].as_ref().unwrap().id, "b");
    }

    #[test]
    fn replaced_endpoint_reconnects_even_without_an_empty_scan() {
        let (_dir, mut m, rx) = harness();
        m.driver.ports = vec!["a".into()];
        m.scan();
        let old = m.driver.opened[0].1.clone();
        old.lock().unwrap().message(note(true));
        m.driver.invalid_once = Some("a".into());
        m.scan();
        assert_eq!(m.driver.opened.len(), 2);
        assert!(m.snapshot.devices[0].connected);
        old.lock().unwrap().message(note(true));
        let events: Vec<_> = rx.try_iter().collect();
        assert_eq!(events.len(), 3);
        assert!(matches!(events[1], Event::Message(e) if e.message == note(false)));
        assert!(matches!(events[2], Event::ResetPort(0)));
    }

    #[test]
    fn device_selection_survives_disconnect_restart_and_duplicate_names() {
        let (dir, mut m, rx) = harness();
        let path = dir.path().join("prefs.json");
        m.driver.ports = vec!["a".into(), "b".into()];
        m.scan();
        m.driver.opened[0].1.lock().unwrap().message(note(true));
        m.set_enabled("a", false, &path);
        m.scan();
        assert!(
            matches!(rx.try_iter().nth(1), Some(Event::Message(e)) if e.message == note(false))
        );
        assert!(!m.snapshot.devices[0].enabled);
        assert!(m.snapshot.devices[1].connected);
        m.driver.ports.clear();
        m.scan();
        assert_eq!(m.snapshot.devices.len(), 1);
        assert_eq!(m.snapshot.devices[0].status, "Offline · disabled");
        let mut restarted = Manager::new(FakeDriver::default(), m.sink.clone(), &path, true);
        restarted.driver.ports = vec!["b".into(), "a".into()];
        restarted.scan();
        assert!(restarted.snapshot.devices[0].connected);
        assert!(!restarted.snapshot.devices[1].connected);
        restarted.set_enabled("a", true, &path);
        restarted.scan();
        assert!(restarted.snapshot.devices[1].connected);
        assert!(Preferences::load(&path).unwrap().disabled.is_empty());
    }

    #[test]
    fn transient_device_ids_never_load_or_save_persistent_selections() {
        let (dir, m, _) = harness();
        let path = dir.path().join("prefs.json");
        let prefs = Preferences {
            disabled: BTreeMap::from([("a".into(), "Old device".into())]),
        };
        prefs.save(&path).unwrap();
        let mut m = Manager::new(FakeDriver::default(), m.sink.clone(), &path, false);
        m.driver.ports = vec!["a".into()];
        m.scan();
        assert!(m.snapshot.devices[0].connected);
        m.set_enabled("a", false, &path);
        m.scan();
        assert!(!m.snapshot.devices[0].connected);
        m.driver.ports.clear();
        m.scan();
        m.driver.ports.push("a".into());
        m.scan();
        assert!(m.snapshot.devices[0].connected);
        assert_eq!(
            Preferences::load(&path).unwrap().disabled["a"],
            "Old device"
        );
    }

    #[test]
    fn scan_connect_and_save_failures_are_visible_and_recoverable() {
        let (dir, mut m, _) = harness();
        m.driver.ports = vec!["a".into()];
        m.driver.fail_connect = true;
        m.scan();
        assert!(m.snapshot.devices[0].status.contains("busy"));
        m.driver.fail_connect = false;
        m.scan();
        assert!(m.snapshot.devices[0].connected);
        m.driver.fail_scan = true;
        m.scan();
        assert!(m.snapshot.error.contains("scan failed"));
        assert!(m.connections[0].is_some());
        m.driver.fail_scan = false;
        // An existing directory cannot be atomically replaced by a file.
        m.set_enabled("a", false, dir.path());
        m.scan();
        assert!(m.snapshot.error.contains("Could not save"));
        assert!(m.snapshot.devices[0].enabled);
        m.set_enabled("a", false, &dir.path().join("prefs.json"));
        m.scan();
        assert!(m.snapshot.error.is_empty());
        assert!(!m.snapshot.devices[0].connected);
    }

    #[test]
    fn input_limit_recovers_when_a_slot_becomes_available() {
        let (_dir, mut m, _) = harness();
        m.driver.ports = (0..MAX_INPUT_PORTS + 1).map(|i| i.to_string()).collect();
        m.scan();
        assert!(!m.snapshot.devices[MAX_INPUT_PORTS].connected);
        assert!(m.snapshot.devices[MAX_INPUT_PORTS].status.contains("limit"));
        m.driver.ports.remove(0);
        m.scan();
        assert!(m.snapshot.devices.iter().all(|d| d.connected));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn coremidi_hotplug_delivers_notes_and_disconnect_cleanup() {
        use midir::os::unix::VirtualOutput;
        let dir = tempfile::tempdir().unwrap();
        let service = Service::start_at(None, dir.path().join("prefs.json")).unwrap();
        let name = format!("eseq-hotplug-test-{}", std::process::id());
        let wait = |predicate: &dyn Fn(&Event) -> bool| {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            loop {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                let event = service
                    .rx
                    .recv_timeout(remaining)
                    .expect("MIDI event before deadline");
                if predicate(&event) {
                    return event;
                }
            }
        };
        wait(&|event| matches!(event, Event::Snapshot(_)));
        let mut output = midir::MidiOutput::new(&name)
            .unwrap()
            .create_virtual(&name)
            .unwrap();
        wait(
            &|event| matches!(event, Event::Snapshot(s) if s.devices.iter().any(|d| d.name == name && d.connected)),
        );
        output.send(&[0x93, 60, 100]).unwrap();
        let Event::Message(on) = wait(&|event| {
            matches!(event, Event::Message(e) if matches!(e.message,
            MidiMessage::Note { channel: 3, note: MidiNoteEvent { note: 60, on: true, .. } }))
        }) else {
            unreachable!()
        };
        drop(output);
        wait(
            &|event| matches!(event, Event::Message(e) if e.port == on.port && e.message == note(false)),
        );
        wait(&|event| matches!(event, Event::ResetPort(port) if *port == on.port));
    }
}
