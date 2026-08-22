# GeneralUser-GS.sf2

**What it is:** the complete GeneralUser GS 2.0.3 General MIDI/Roland GS
SoundFont bank (32,319,396 bytes). It is the application's canonical bundled
instrument bank: it supplies both the default piano and the accordion selected
by the global instrument control, as well as its full GM/GS preset collection.
Do not replace it with TimGM6mb or a reduced bank simply to shrink the app
bundle.

**Source and version:** S. Christian Collins, GeneralUser GS 2.0.3,
documentation revision 6 (2026-02-23). This exact copy was taken from upstream
commit [`6f2014e815237de02e26e793f8c66c796ba66db5`](https://github.com/mrbumpy409/GeneralUser-GS/tree/6f2014e815237de02e26e793f8c66c796ba66db5),
whose source archive has SHA-256
`f8bcd7722f10e00e530b019d5b162eeb5a59655f4a226225ca0459a7d74fb53f`.
The author’s homepage is <https://www.schristiancollins.com/generaluser.php>.

**Why this bank:** the project deliberately prioritises a complete, higher
quality GM/GS palette over the former ~6 MB TimGM6mb bank. GeneralUser GS
contains 261 presets and 13 drum kits while its SoundFont itself is about
31 MB. The extra bundle size is intentional product scope, not accidental
bloat.

**Synth compatibility:** GeneralUser GS 2.0.3 makes substantial use of
SoundFont modulators. Its author lists FluidSynth 2.3+ and other complete
SoundFont implementations as fully compatible, and warns that engines without
modulator support render many presets incorrectly. The current `rustysynth`
adapter parses and plays this file, but does not implement SoundFont modulators.
It is therefore acceptable only as a temporary compatibility path for the
currently exposed piano and accordion; before exposing or promising faithful
playback of the entire bank, replace the adapter with a complete engine (for
example FluidSynth 2.3+) and manually audition the presets. Do not solve that
compatibility gap by reverting this asset.

## Upstream licence (GeneralUser GS v2.0)

> You may use GeneralUser GS without restriction for your own music creation,
> private or commercial. This SoundFont bank is provided to the community free
> of charge. Please feel free to use it in your software projects, and to
> modify the SoundFont bank or its packaging to suit your needs.
>
> GeneralUser GS inherits the usage rights of the samples contained within, all
> of which allow full use in music production, including the ability to make
> profit from musical recordings created with GeneralUser GS.
>
> Many of the samples are original, but some were taken from other banks freely
> (and legally) available on the Internet from various SoundFont websites.
> Because GeneralUser GS originated as a personal project with no intention for
> publication, I cannot be 100% sure where all of the samples originated,
> although I do know that none of them came from commercially published
> SoundFont packages or sample CDs. Regardless, many "free" SoundFonts
> available on the web may indeed contain samples of questionable origin. My
> understanding of the copyrights of all samples is only as good as the
> information provided by the original sources. If you become aware of any
> restricted samples being used in GeneralUser GS, please let me know so I can
> replace them.
>
> This uncertainty may concern you if you intend to use GeneralUser GS in a
> commercial software product. That being said, I have never received any
> complaint regarding sample ownership since I published the original
> GeneralUser GS back in 2000, and as far as I am aware, neither have any of
> the companies creating commercial software products using GeneralUser GS.
>
> If you plan to feature GeneralUser GS on your own website, please do not link
> directly to my download files. Either link to my website, or provide your own
> local copy instead.
