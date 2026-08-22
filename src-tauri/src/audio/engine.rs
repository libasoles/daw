//! Wires the `Synth` port to real audio hardware via `cpal`, split across
//! the synth thread and the real-time callback thread described in the
//! `audio` module doc comment.

use std::fmt;
use std::thread;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use daw_core::{
    is_bar_accent,
    ports::{InstrumentId, Synth},
};
use rtrb::RingBuffer;

use super::synth::{RustySynth, RustySynthError};

/// A request sent from the IPC thread to the synth thread. Deliberately
/// minimal today — there is no timeline to schedule from until H3 — so this
/// exists to let a debug command prove sound reaches the speakers. Later
/// tickets extend this enum (e.g. with a schedule snapshot) rather than the
/// thread split or the queues that carry it.
pub enum EngineCommand {
    Configure {
        instrument: InstrumentId,
        reverb: u8,
        bpm: u16,
        time_signature: (u8, u8),
        metronome_enabled: bool,
    },
    NoteOn {
        pitch: u8,
        velocity: u8,
        received_at: Option<Instant>,
    },
    NoteOff {
        pitch: u8,
    },
    /// Plays a take back in isolation (issue #8): the shell converts each
    /// [`daw_core::RecordedNote`] into two of these (one on, one off) and
    /// sends the lot at once. Firing them at the right frame is the synth
    /// thread's job — see its per-sample loop, which already derives "what
    /// pulse is this frame" for the metronome and reuses that math here.
    PlaySchedule(Vec<ScheduledEvent>),
}

/// One note on/off to fire at a given pulse, relative to whenever the
/// schedule carrying it started playing. See [`EngineCommand::PlaySchedule`].
pub struct ScheduledEvent {
    pub at_pulse: u64,
    pub pitch: u8,
    pub velocity: u8,
    pub is_on: bool,
}

/// A schedule the synth thread is actively working through: the events,
/// sorted by `at_pulse`, the render-frame they started at (pulse zero), and
/// how far through the list it has gotten.
struct ActiveSchedule {
    events: Vec<ScheduledEvent>,
    started_at_frame: u64,
    next_index: usize,
}

/// Interleaved stereo samples buffered between the synth thread and the
/// real-time callback. This is deliberately only about 12 ms at 44.1 kHz
/// (512 stereo frames): a larger pre-rendered cushion would delay every live
/// MIDI note by its entire depth before the callback could hear it.
const AUDIO_QUEUE_CAPACITY: usize = 1 << 10;

/// Pending note on/off requests the synth thread hasn't drained yet. Debug
/// triggering is the only source of these today, so this only needs to be
/// larger than a person could plausibly click in one batch.
const COMMAND_QUEUE_CAPACITY: usize = 256;

/// Frames rendered per synth-thread iteration. Small enough that the thread
/// checks for new commands frequently, large enough to keep per-call
/// overhead low.
const RENDER_CHUNK_FRAMES: usize = 64;

/// Everything that can stop the audio engine from starting. Per the spec,
/// none of these should crash the app — see `play_test_note` in
/// `src-tauri/src/lib.rs`, which surfaces the equivalent at call time.
#[derive(Debug)]
pub enum AudioEngineError {
    NoOutputDevice,
    NoOutputConfig(cpal::Error),
    BuildStream(cpal::Error),
    PlayStream(cpal::Error),
    Synth(RustySynthError),
}

impl fmt::Display for AudioEngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AudioEngineError::NoOutputDevice => {
                write!(f, "no audio output device is available")
            }
            AudioEngineError::NoOutputConfig(err) => {
                write!(f, "could not read the output device's configuration: {err}")
            }
            AudioEngineError::BuildStream(err) => {
                write!(f, "could not open the audio stream: {err}")
            }
            AudioEngineError::PlayStream(err) => {
                write!(f, "could not start the audio stream: {err}")
            }
            AudioEngineError::Synth(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for AudioEngineError {}

/// A running audio engine: a handle for sending note on/off requests to the
/// synth thread. Dropping this handle does not stop playback — the synth
/// thread and the `cpal` stream are deliberately detached from it (see the
/// `audio` module doc comment for why the stream itself can't live here).
pub struct AudioEngine {
    commands: rtrb::Producer<EngineCommand>,
}

impl AudioEngine {
    /// Starts the audio engine: opens the default output device, spins up
    /// the synth thread, and begins playing immediately. Returns an error
    /// rather than panicking when no output device is available or the
    /// bundled SoundFont fails to load, per the spec's acceptance criterion
    /// that a missing device is reported, not a crash.
    pub fn start() -> Result<Self, AudioEngineError> {
        let (command_producer, command_consumer) = RingBuffer::new(COMMAND_QUEUE_CAPACITY);

        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or(AudioEngineError::NoOutputDevice)?;
        let config = device
            .default_output_config()
            .map_err(AudioEngineError::NoOutputConfig)?;
        let sample_rate = config.sample_rate() as i32;
        let channels = config.channels() as usize;
        let stream_config: cpal::StreamConfig = config.into();

        let synth =
            RustySynth::with_bundled_sound_font(sample_rate).map_err(AudioEngineError::Synth)?;

        let (audio_producer, mut audio_consumer) = RingBuffer::<f32>::new(AUDIO_QUEUE_CAPACITY);
        spawn_synth_thread(synth, sample_rate as u32, command_consumer, audio_producer);

        let stream = device
            .build_output_stream(
                stream_config,
                move |data: &mut [f32], _| {
                    // The real-time audio thread: only pops already-rendered
                    // samples and copies them out. No lock, no allocation,
                    // no call into `Synth` or `DawCore`.
                    for frame in data.chunks_mut(channels) {
                        let left = audio_consumer.pop().unwrap_or(0.0);
                        let right = audio_consumer.pop().unwrap_or(0.0);
                        let is_mono = frame.len() == 1;
                        if let Some(first) = frame.first_mut() {
                            *first = if is_mono { 0.5 * (left + right) } else { left };
                        }
                        if frame.len() > 1 {
                            frame[1] = right;
                        }
                        for extra in frame.iter_mut().skip(2) {
                            *extra = 0.0;
                        }
                    }
                },
                |err| eprintln!("audio output stream error: {err}"),
                None,
            )
            .map_err(AudioEngineError::BuildStream)?;

        stream.play().map_err(AudioEngineError::PlayStream)?;

        // `cpal::Stream` is not `Send` on every backend, so it must stay on
        // the thread that created it. This thread's only job from here is to
        // keep `stream` alive (dropping it stops playback) for the life of
        // the process; the actual rendering happens on the synth thread and
        // inside the callback above.
        thread::spawn(move || {
            let _stream = stream;
            loop {
                thread::park();
            }
        });

        Ok(Self {
            commands: command_producer,
        })
    }

    /// Requests that `pitch` start sounding with the current global
    /// instrument. Non-blocking:
    /// if the synth thread has fallen behind and the command queue is full,
    /// this drops the request and reports it rather than blocking the
    /// caller (an IPC thread).
    pub fn note_on(&mut self, pitch: u8, velocity: u8) -> Result<(), EngineCommandDropped> {
        self.commands
            .push(EngineCommand::NoteOn {
                pitch,
                velocity,
                received_at: None,
            })
            .map_err(|_| EngineCommandDropped)
    }

    /// The live MIDI path. The synth thread logs its receipt time against the
    /// native callback timestamp, giving a repeatable internal latency metric
    /// without pretending to measure keyboard scanning, driver, DAC, or air.
    pub fn note_on_live(
        &mut self,
        pitch: u8,
        velocity: u8,
        received_at: Instant,
    ) -> Result<(), EngineCommandDropped> {
        self.commands
            .push(EngineCommand::NoteOn {
                pitch,
                velocity,
                received_at: Some(received_at),
            })
            .map_err(|_| EngineCommandDropped)
    }

    /// Requests that `pitch` stop sounding. See [`Self::note_on`].
    pub fn note_off(&mut self, pitch: u8) -> Result<(), EngineCommandDropped> {
        self.commands
            .push(EngineCommand::NoteOff { pitch })
            .map_err(|_| EngineCommandDropped)
    }

    /// Requests that the synth thread play `events` back, timed against its
    /// own running pulse clock rather than the caller's. Non-blocking; see
    /// [`Self::note_on`].
    pub fn play_schedule(
        &mut self,
        events: Vec<ScheduledEvent>,
    ) -> Result<(), EngineCommandDropped> {
        self.commands
            .push(EngineCommand::PlaySchedule(events))
            .map_err(|_| EngineCommandDropped)
    }

    /// Updates the global sound controls on the synth thread. Keeping these
    /// commands alongside note events means every current and future trigger
    /// path receives the same selected instrument and reverb automatically.
    pub fn configure(
        &mut self,
        instrument: InstrumentId,
        reverb: u8,
        bpm: u16,
        time_signature: (u8, u8),
        metronome_enabled: bool,
    ) -> Result<(), EngineCommandDropped> {
        self.commands
            .push(EngineCommand::Configure {
                instrument,
                reverb,
                bpm,
                time_signature,
                metronome_enabled,
            })
            .map_err(|_| EngineCommandDropped)
    }
}

/// The command queue to the synth thread was full, so a note on/off request
/// was dropped rather than blocking the caller.
#[derive(Debug)]
pub struct EngineCommandDropped;

impl fmt::Display for EngineCommandDropped {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "audio engine command queue is full")
    }
}

impl std::error::Error for EngineCommandDropped {}

/// The synth thread: a plain, non-real-time OS thread that owns the `Synth`
/// implementation. It drains pending note on/off requests, renders audio in
/// fixed-size chunks, and pushes the rendered samples for the real-time
/// callback to consume.
fn spawn_synth_thread(
    mut synth: RustySynth,
    sample_rate: u32,
    mut commands: rtrb::Consumer<EngineCommand>,
    mut samples: rtrb::Producer<f32>,
) {
    thread::spawn(move || {
        let mut instrument = 0;
        let mut bpm = 120;
        let mut time_signature = (3, 4);
        let mut metronome_enabled = false;
        let mut last_metronome_pulse = None;
        let mut rendered_frames = 0u64;
        let mut click_frames_remaining = 0usize;
        let mut click_amplitude = 0.0;
        let mut schedule: Option<ActiveSchedule> = None;
        let mut held_from_schedule: Vec<u8> = Vec::new();
        let mut left = [0f32; RENDER_CHUNK_FRAMES];
        let mut right = [0f32; RENDER_CHUNK_FRAMES];
        loop {
            while let Ok(command) = commands.pop() {
                match command {
                    EngineCommand::Configure {
                        instrument: next_instrument,
                        reverb,
                        bpm: next_bpm,
                        time_signature: next_time_signature,
                        metronome_enabled: next_metronome_enabled,
                    } => {
                        instrument = next_instrument;
                        synth.set_reverb(reverb);
                        if metronome_enabled && !next_metronome_enabled {
                            last_metronome_pulse = None;
                            click_frames_remaining = 0;
                        }
                        metronome_enabled = next_metronome_enabled;
                        bpm = next_bpm;
                        time_signature = next_time_signature;
                    }
                    EngineCommand::NoteOn {
                        pitch,
                        velocity,
                        received_at,
                    } => {
                        synth.note_on(instrument, pitch, velocity, 0);
                        if let Some(received_at) = received_at {
                            eprintln!(
                                "MIDI internal latency: note {pitch} reached synth thread in {:?}",
                                received_at.elapsed()
                            );
                        }
                    }
                    EngineCommand::NoteOff { pitch } => synth.note_off(instrument, pitch, 0),
                    EngineCommand::PlaySchedule(mut events) => {
                        // A new schedule pre-empts whatever the previous one
                        // left sounding, so a fast re-trigger can never
                        // strand a note on.
                        for pitch in held_from_schedule.drain(..) {
                            synth.note_off(instrument, pitch, 0);
                        }
                        events.sort_by_key(|event| event.at_pulse);
                        schedule = Some(ActiveSchedule {
                            events,
                            started_at_frame: rendered_frames,
                            next_index: 0,
                        });
                    }
                }
            }

            synth.render(&mut left, &mut right);

            for (l, r) in left.iter_mut().zip(right.iter_mut()) {
                if let Some(active) = schedule.as_mut().filter(|_| bpm != 0) {
                    // Same frames-to-pulse conversion as the metronome click
                    // below, just anchored at the schedule's own start frame
                    // instead of the stream's.
                    let elapsed_frames = rendered_frames - active.started_at_frame;
                    let pulse = elapsed_frames.saturating_mul(u64::from(bpm))
                        / (u64::from(sample_rate) * 60);
                    while active
                        .events
                        .get(active.next_index)
                        .is_some_and(|event| event.at_pulse <= pulse)
                    {
                        let event = &active.events[active.next_index];
                        if event.is_on {
                            synth.note_on(instrument, event.pitch, event.velocity, pulse);
                            held_from_schedule.push(event.pitch);
                        } else {
                            synth.note_off(instrument, event.pitch, pulse);
                            held_from_schedule.retain(|&pitch| pitch != event.pitch);
                        }
                        active.next_index += 1;
                    }
                    if active.next_index >= active.events.len() {
                        schedule = None;
                    }
                }

                if metronome_enabled && bpm != 0 {
                    // This is the inverse of `daw_core::pulse_elapsed_time`:
                    // elapsed rendered frames determine the current pulse.
                    // It runs here, never in the real-time callback.
                    let pulse = rendered_frames.saturating_mul(u64::from(bpm))
                        / (u64::from(sample_rate) * 60);
                    if last_metronome_pulse != Some(pulse) {
                        last_metronome_pulse = Some(pulse);
                        click_frames_remaining =
                            usize::try_from(sample_rate / 125).unwrap_or(1).max(1);
                        click_amplitude = if is_bar_accent(pulse, time_signature) {
                            0.35
                        } else {
                            0.2
                        };
                    }
                }

                if click_frames_remaining != 0 {
                    // A short alternating, decaying impulse is enough to be
                    // audible as a click without another audio dependency.
                    let click = if click_frames_remaining.is_multiple_of(2) {
                        click_amplitude
                    } else {
                        -click_amplitude
                    } * (click_frames_remaining as f32
                        / (sample_rate / 125).max(1) as f32);
                    *l += click;
                    *r += click;
                    click_frames_remaining -= 1;
                }

                // This thread is not real-time: briefly waiting rather than
                // dropping samples when the queue is full cannot stall the
                // audio callback, which only ever reads.
                while samples.push(*l).is_err() {
                    thread::sleep(Duration::from_micros(200));
                }
                while samples.push(*r).is_err() {
                    thread::sleep(Duration::from_micros(200));
                }
                rendered_frames = rendered_frames.saturating_add(1);
            }
        }
    });
}
