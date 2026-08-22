//! The real [`daw_core::ports::Synth`] adapter: a General MIDI synthesiser
//! backed by `rustysynth` and a bundled SoundFont.
//!
//! This is deliberately not unit-tested — the spec (issue #1) is explicit
//! that `rustysynth` sits behind the port and is "exercised by hand," not
//! asserted on in `cargo test`, since there is no sound hardware in CI to
//! confirm against. What *is* verified here is that this adapter compiles
//! against the port and is wired correctly; whether notes actually sound is
//! for a human to confirm (see the ticket's report for exactly how).

use std::fmt;
use std::io::Cursor;
use std::sync::Arc;

use daw_core::ports::{InstrumentId, Synth};
use rustysynth::{SoundFont, SoundFontError, Synthesizer, SynthesizerError, SynthesizerSettings};

/// The bundled General MIDI SoundFont. See
/// `src-tauri/assets/soundfonts/NOTICE.md` for its license and source.
///
/// Embedded with `include_bytes!` at compile time, rather than resolved as a
/// Tauri bundle resource at runtime, so it is guaranteed present in release
/// builds without a separate resource-path lookup — the acceptance criterion
/// this satisfies is "a piano SoundFont is bundled with the application and
/// available in release builds." GeneralUser GS is a complete GM/GS bank
/// (~31 MB), intentionally chosen over the former small TimGM bank; see the
/// NOTICE for the fixed upstream version, licence, and compatibility caveat.
const SOUND_FONT_BYTES: &[u8] = include_bytes!("../../assets/soundfonts/GeneralUser-GS.sf2");

/// The two instruments exposed by the initial global selector. These remain
/// opaque at the `Synth` port; adding another one is a mapping/data change
/// here, not a trait or engine redesign. GM program numbers are zero-based.
const PIANO_PROGRAM: i32 = 0;
const ACCORDION_PROGRAM: i32 = 21;
const GM_CHANNEL: i32 = 0;
const MIDI_REVERB_CONTROLLER: i32 = 0x5B;

/// Everything that can go wrong constructing a [`RustySynth`]: either the
/// bundled bytes don't parse as a SoundFont, or `rustysynth` rejects the
/// settings derived from them.
#[derive(Debug)]
pub enum RustySynthError {
    SoundFont(SoundFontError),
    Synthesizer(SynthesizerError),
}

impl fmt::Display for RustySynthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RustySynthError::SoundFont(err) => {
                write!(f, "failed to load the bundled SoundFont: {err:?}")
            }
            RustySynthError::Synthesizer(err) => {
                write!(f, "failed to start the synthesizer: {err:?}")
            }
        }
    }
}

impl std::error::Error for RustySynthError {}

/// A General MIDI synthesiser, adapting `rustysynth::Synthesizer` to the
/// `daw_core::ports::Synth` port.
///
/// Instruments are identified by an opaque [`InstrumentId`] at the port
/// boundary; today every id is routed to MIDI channel 0, which plays the
/// bundled SoundFont's default preset (a piano) since the instrument
/// dropdown (issue #5) does not exist yet. Extending this to route different
/// ids to different programs/channels is a change to this one mapping, not a
/// change to the `Synth` trait.
pub struct RustySynth {
    inner: Synthesizer,
}

impl RustySynth {
    /// Loads the bundled SoundFont and starts a synthesizer rendering at
    /// `sample_rate` (typically the output device's own sample rate, so no
    /// resampling is needed downstream).
    pub fn with_bundled_sound_font(sample_rate: i32) -> Result<Self, RustySynthError> {
        let mut reader = Cursor::new(SOUND_FONT_BYTES);
        let sound_font = Arc::new(SoundFont::new(&mut reader).map_err(RustySynthError::SoundFont)?);
        let settings = SynthesizerSettings::new(sample_rate);
        let inner =
            Synthesizer::new(&sound_font, &settings).map_err(RustySynthError::Synthesizer)?;
        Ok(Self { inner })
    }
}

impl Synth for RustySynth {
    fn set_reverb(&mut self, reverb: u8) {
        // rustysynth exposes General MIDI control changes directly. CC 91 is
        // reverb send, with the MIDI range 0..=127; project state uses the
        // musician-facing 0..=100 percent range.
        let midi_reverb = i32::from(reverb.min(100)) * 127 / 100;
        self.inner
            .process_midi_message(GM_CHANNEL, 0xB0, MIDI_REVERB_CONTROLLER, midi_reverb);
    }

    fn note_on(&mut self, instrument: InstrumentId, pitch: u8, velocity: u8, _pulse: u64) {
        // There is no timeline yet to make `pulse` meaningful; it is
        // received and ignored here so the port's shape doesn't have to
        // change once one exists.
        self.inner.process_midi_message(
            GM_CHANNEL,
            0xC0,
            match instrument {
                1 => ACCORDION_PROGRAM,
                _ => PIANO_PROGRAM,
            },
            0,
        );
        self.inner
            .note_on(GM_CHANNEL, pitch as i32, velocity as i32);
    }

    fn note_off(&mut self, _instrument: InstrumentId, pitch: u8, _pulse: u64) {
        self.inner.note_off(GM_CHANNEL, pitch as i32);
    }

    fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.inner.render(left, right);
    }
}
