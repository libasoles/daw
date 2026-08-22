# TimGM6mb.sf2

**What it is:** a small, complete General MIDI SoundFont bank (~6 MB). It
supplies the piano sound the app plays through the `Synth` port (issue #4)
until the instrument dropdown (issue #5) adds an accordion alongside it.

**Source and authorship:** created by Tim Brechbill (2004), with later
contributions from David Bolton (2010). It shipped as the bundled instrument
bank in MuseScore 1.x, and remains bundled today by several other open-source
projects for the same reason this one does — a small, complete, freely
redistributable General MIDI bank — including
[`craffel/pretty-midi`](https://github.com/craffel/pretty-midi) (the exact
copy in this directory was fetched from that repository) and
[`gleitz/midi2audio`](https://github.com/gleitz/midi2audio). The Debian
project distributes it as the `timgm6mb-soundfont` package.

**License:** GNU General Public License, version 2 or later. Per the
copyright holder's own statement (recorded in Debian's copyright file for the
`timgm6mb-soundfont` package), every sample in the bank is either original to
Tim Brechbill, drawn from public-domain sources, or itself covered by the
GPL. This is a data asset bundled with the application, not code linked into
it, so its license governs redistribution of the `.sf2` file itself; it does
not extend to the surrounding application.

**Why this SoundFont and not another:** bundle size is called out explicitly
as a real constraint in the spec (issue #1). `TimGM6mb.sf2` is a small
fraction of the size of the more commonly recommended free banks —
`GeneralUser GS` (~30 MB) or `FluidR3_GM` (>100 MB) — while still being a
complete General MIDI bank, and it has prior art as a bundled asset in other
small open-source projects for exactly this reason.

**Verifying it manually:** the file is a standard RIFF/SoundFont container;
`file src-tauri/assets/soundfonts/TimGM6mb.sf2` reports `RIFF (little-endian)
data, SoundFont/Bank`.
