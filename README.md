# Ómhemill

*óm* (resonance) + *hemill* (brake) — the resonance brake.

A spectral resonance suppressor: a per-bin compressor across the whole spectrum that can tame
resonances, lift buried detail, or both at once. You aim it with up to eight bands drawn on the
spectrum, the way you would shape an EQ, and keep the rest of the controls to a handful.

This is a fork of Robbert van der Helm's
[Spectral Compressor](https://github.com/robbert-vdh/spectral-compressor) (GPL-3.0-or-later). The
STFT engine and the per-bin upwards/downwards compressor bank are his; the band model, the editor
and the presets are new. The editor uses the graphite (dark) edition of the
[Sungam](https://sungammagnus.com) design system, the same one as Beygla and Samplebrain. See
`COPYING` for the full license text and the original copyright headers left intact in the source
files.

## The display

The white line is **sensitivity**. It sits flat at 0 dB until a band bends it: above zero the brake
bites harder there, below zero it leaves that range alone. Behind it is the input spectrum, and the
amber between the input and the output is what the brake is taking away right now (a lighter amber
where it lifts).

- **Double-click** empty space to add a band there: a cut at the far edges, a shelf near them, a
  bell anywhere else.
- **Drag** a band for frequency and sensitivity. Hold **shift** for fine moves.
- **Scroll** over a band to change its width.
- **Alt-drag** a band to solo it while you sweep.
- **Double-click** a band to switch it off. Click empty space to deselect.
- **Hover** for the frequency, nearest note, input level and gain change at that point.

## Bands

Selecting a band, on the display or in the chip row below it, opens its panel.

- **Shape** — low cut, low shelf, bell, high shelf, high cut. The cuts do not filter the audio; they
  fence off where the brake works at all.
- **Frequency**, **Width** (a bell's width, a shelf's slope, a cut's softness), and
  **Sensitivity** (±18 dB).
- **This band only** — scale the global controls inside the band: **Sharpness** and **Speed**
  (×0.25 to ×4), **Max change** (the most gain change the band allows), and **Action** (follow the
  header, or suppress, lift or do nothing here).
- **Solo** plays only the band's range. With Delta on, you hear only what that band removes.

## The rest

- **Action** — Suppress, Lift or Both.
- **Listen to** — Self, Sidechain (the sidechain sets the curve) or Duck (the sidechain drives the
  brake).
- **Detect** — Depth (how far below the signal the brake sits), Sharpness (the ratio, for both
  directions) and Selectivity (the knee).
- **Time** — Attack and Release.
- **Output** — Mix and Trim.
- **Resolution** — Fast, Balanced, Fine or Ultra: finer separates resonances more cleanly, at the
  cost of latency. Click to step forward, right-click to step back.
- **Delta** — hear only what the brake changes.
- **Bypass** — the host's bypass.

## Presets

The preset bar steps through the factory presets and your own. Edit the name and press **Save** to
store the current settings as a user preset. User presets are JSON files in:

- macOS: `~/Library/Application Support/Sungam/Omhemill/Presets`
- Windows: `%APPDATA%\Sungam\Omhemill\Presets`
- Linux: `$XDG_DATA_HOME/sungam/omhemill/presets` (or `~/.local/share/...`)

Loading a preset sets every parameter, so nothing is left over from before. It never touches
Delta or Bypass.

## Building

```shell
cargo xtask bundle omhemill --release
```

This produces `target/bundled/Omhemill.vst3` and `target/bundled/Omhemill.clap`.

### Looking at the GUI without a host

There is a windowed preview build. It is behind a feature flag because nih-plug's standalone
wrapper links against `libjack`, and the plugin itself must not pick that dependency up or hosts
will refuse to load it:

```shell
RUSTFLAGS="-L /usr/local/opt/jack/lib" cargo run --release --bin omhemill_standalone --features standalone -- --backend dummy
```

The `-L` is needed on macOS because Homebrew leaves a stale `/usr/local/lib/libjack.dylib` symlink
pointing at a framework that is not installed; the real library lives in the Cellar.

## Related

- [Concept doc and web prototype](../..) — the design rationale, parameter mapping, and a
  real-time Web Audio implementation of the same idea.
