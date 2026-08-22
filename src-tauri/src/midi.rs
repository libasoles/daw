//! Native MIDI input adapter and reconnect controller.
//!
//! WebKit does not provide Web MIDI, so this module is the only route from a
//! physical keyboard to the application. `midir` owns the platform connection
//! (CoreMIDI on macOS); the callback immediately translates only note events
//! into the core's hardware-neutral `MidiInput` port types.

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use daw_core::ports::{
    MidiDevice, MidiEvent, MidiEventHandler, MidiEventKind, MidiInput, MidiInputError,
};
use midir::{Ignore, MidiInput as MidirInput, MidiInputConnection, MidiInputPort};
use tauri::{AppHandle, Manager};

use crate::AudioEngineHandle;

const CLIENT_NAME: &str = "daw MIDI input";
const RECONNECT_INTERVAL: Duration = Duration::from_secs(1);
const PREFERENCE_FILE: &str = "midi-input-device-id";

/// The data returned to the device picker. It is intentionally app state,
/// not `ProjectState`: a keyboard is attached to a particular machine.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MidiStatus {
    pub devices: Vec<MidiDevice>,
    pub selected_device_id: Option<String>,
    pub message: String,
}

/// Real implementation of `daw_core::ports::MidiInput`. A `midir` connection
/// must be retained for the callback to remain active, so it is owned here.
pub struct NativeMidiInput {
    connection: Option<MidiInputConnection<()>>,
}

impl NativeMidiInput {
    pub fn new() -> Self {
        Self { connection: None }
    }

    pub fn disconnect(&mut self) {
        self.connection.take();
    }

    fn source() -> Result<MidirInput, MidiInputError> {
        let mut input = MidirInput::new(CLIENT_NAME)
            .map_err(|error| MidiInputError(format!("could not create MIDI input: {error}")))?;
        input.ignore(Ignore::None);
        Ok(input)
    }

    fn id(
        input: &MidirInput,
        port: &MidiInputPort,
        index: usize,
    ) -> Result<String, MidiInputError> {
        input
            .port_name(port)
            .map(|name| format!("{name}#{index}"))
            .map_err(|error| MidiInputError(format!("could not name MIDI input: {error}")))
    }

    fn port_for_id(input: &MidirInput, device_id: &str) -> Result<MidiInputPort, MidiInputError> {
        input
            .ports()
            .into_iter()
            .enumerate()
            .find_map(|(index, port)| {
                (Self::id(input, &port, index).ok().as_deref() == Some(device_id)).then_some(port)
            })
            .ok_or_else(|| MidiInputError(format!("MIDI device not found: {device_id}")))
    }
}

impl MidiInput for NativeMidiInput {
    fn devices(&self) -> Result<Vec<MidiDevice>, MidiInputError> {
        let input = Self::source()?;
        input
            .ports()
            .into_iter()
            .enumerate()
            .map(|(index, port)| {
                let name = input.port_name(&port).map_err(|error| {
                    MidiInputError(format!("could not name MIDI input: {error}"))
                })?;
                Ok(MidiDevice {
                    id: format!("{name}#{index}"),
                    name,
                })
            })
            .collect()
    }

    fn subscribe(
        &mut self,
        device_id: &str,
        handler: MidiEventHandler,
    ) -> Result<(), MidiInputError> {
        self.disconnect();
        let input = Self::source()?;
        let port = Self::port_for_id(&input, device_id)?;
        let subscribed_at = Instant::now();
        self.connection = Some(
            input
                .connect(
                    &port,
                    CLIENT_NAME,
                    move |_native_timestamp, message, _| {
                        if let Some(kind) = parse_note_message(message) {
                            handler(MidiEvent {
                                timestamp: subscribed_at.elapsed(),
                                kind,
                            });
                        }
                    },
                    (),
                )
                .map_err(|error| {
                    MidiInputError(format!("could not subscribe to MIDI input: {error}"))
                })?,
        );
        Ok(())
    }
}

fn parse_note_message(message: &[u8]) -> Option<MidiEventKind> {
    let (&status, rest) = message.split_first()?;
    let (&pitch, rest) = rest.split_first()?;
    let velocity = rest.first().copied()?;
    match status & 0xF0 {
        0x90 if velocity > 0 => Some(MidiEventKind::NoteOn { pitch, velocity }),
        0x80 | 0x90 => Some(MidiEventKind::NoteOff { pitch }),
        _ => None,
    }
}

/// Tauri-managed MIDI state. Polling is used because `midir` does not offer a
/// portable hot-plug callback; an unplug drops the connection and a replug of
/// the remembered id is subscribed on the next poll.
pub struct MidiInputHandle(pub std::sync::Mutex<MidiController>);

pub struct MidiController {
    input: NativeMidiInput,
    selected_device_id: Option<String>,
    connected_device_id: Option<String>,
    status: MidiStatus,
}

impl MidiController {
    pub fn new(selected_device_id: Option<String>) -> Self {
        Self {
            input: NativeMidiInput::new(),
            selected_device_id,
            connected_device_id: None,
            status: MidiStatus {
                devices: Vec::new(),
                selected_device_id: None,
                message: "Checking MIDI inputs…".into(),
            },
        }
    }

    pub fn refresh(&mut self, app: &AppHandle) {
        let devices = match self.input.devices() {
            Ok(devices) => devices,
            Err(error) => {
                self.input.disconnect();
                self.connected_device_id = None;
                self.status = MidiStatus {
                    devices: Vec::new(),
                    selected_device_id: self.selected_device_id.clone(),
                    message: format!("Could not read MIDI inputs: {error}"),
                };
                return;
            }
        };
        let selected_present = self
            .selected_device_id
            .as_ref()
            .is_some_and(|id| devices.iter().any(|device| &device.id == id));

        if !selected_present {
            self.input.disconnect();
            self.connected_device_id = None;
            self.status = MidiStatus {
                message: if devices.is_empty() {
                    "No MIDI input device is available.".into()
                } else if self.selected_device_id.is_some() {
                    "Selected MIDI input is unavailable; reconnect the keyboard to resume.".into()
                } else {
                    "Choose a MIDI input device.".into()
                },
                devices,
                selected_device_id: self.selected_device_id.clone(),
            };
            return;
        }

        let selected = self.selected_device_id.clone().expect("checked above");
        if self.connected_device_id.as_deref() != Some(&selected) {
            let callback_app = app.clone();
            let result = self.input.subscribe(
                &selected,
                Box::new(move |event| route_live_event(&callback_app, event)),
            );
            if let Err(error) = result {
                self.connected_device_id = None;
                self.status = MidiStatus {
                    devices,
                    selected_device_id: Some(selected),
                    message: format!("Could not connect MIDI input: {error}"),
                };
                return;
            }
            self.connected_device_id = Some(selected);
        }
        self.status = MidiStatus {
            devices,
            selected_device_id: self.selected_device_id.clone(),
            message: "MIDI input connected.".into(),
        };
    }

    pub fn select(&mut self, app: &AppHandle, device_id: String) -> Result<(), String> {
        let devices = self.input.devices().map_err(|error| error.to_string())?;
        if !devices.iter().any(|device| device.id == device_id) {
            return Err("The selected MIDI input is no longer available.".into());
        }
        self.selected_device_id = Some(device_id.clone());
        save_selected_device(app, &device_id)?;
        self.connected_device_id = None;
        self.input.disconnect();
        self.refresh(app);
        Ok(())
    }

    pub fn status(&self) -> MidiStatus {
        self.status.clone()
    }
}

fn route_live_event(app: &AppHandle, event: MidiEvent) {
    let received_at = Instant::now();
    let engine = app.state::<AudioEngineHandle>();
    let mut engine = engine.0.lock().expect("audio engine mutex poisoned");
    let Some(engine) = engine.as_mut() else {
        eprintln!("MIDI input received but no audio output is available");
        return;
    };
    let result = match event.kind {
        MidiEventKind::NoteOn { pitch, velocity } => {
            engine.note_on_live(pitch, velocity, received_at)
        }
        MidiEventKind::NoteOff { pitch } => engine.note_off(pitch),
    };
    if let Err(error) = result {
        eprintln!("could not forward MIDI note to audio engine: {error}");
    }
}

pub fn load_selected_device(app: &AppHandle) -> Option<String> {
    fs::read_to_string(preference_path(app)?)
        .ok()
        .map(|id| id.trim().to_owned())
        .filter(|id| !id.is_empty())
}

fn save_selected_device(app: &AppHandle, device_id: &str) -> Result<(), String> {
    let path =
        preference_path(app).ok_or_else(|| "could not find the app data directory".to_string())?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("could not create MIDI preference directory: {error}"))?;
    }
    fs::write(path, device_id).map_err(|error| format!("could not save MIDI preference: {error}"))
}

fn preference_path(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_data_dir()
        .ok()
        .map(|dir| dir.join(PREFERENCE_FILE))
}

pub fn spawn_reconnector(app: AppHandle) {
    std::thread::spawn(move || loop {
        {
            let handle = app.state::<MidiInputHandle>();
            let mut controller = handle.0.lock().expect("MIDI input mutex poisoned");
            controller.refresh(&app);
        }
        std::thread::sleep(RECONNECT_INTERVAL);
    });
}

#[cfg(test)]
mod tests {
    use super::parse_note_message;
    use daw_core::ports::MidiEventKind;

    #[test]
    fn translates_note_on_and_note_off_messages() {
        assert_eq!(
            parse_note_message(&[0x91, 60, 80]),
            Some(MidiEventKind::NoteOn {
                pitch: 60,
                velocity: 80
            })
        );
        assert_eq!(
            parse_note_message(&[0x90, 60, 0]),
            Some(MidiEventKind::NoteOff { pitch: 60 })
        );
        assert_eq!(
            parse_note_message(&[0x81, 60, 17]),
            Some(MidiEventKind::NoteOff { pitch: 60 })
        );
    }
}
