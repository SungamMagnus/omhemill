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

The white line is the **threshold**, relative to the main Threshold. It sits flat at 0 dB until a
band bends it: pull a band down and it compresses harder there, the way pulling an EQ band down
cuts; push it up and it leaves that range alone. Behind it is the input spectrum, and the amber
between the input and the output is the gain reduction right now (a lighter amber where upward
compression lifts).

- **Double-click** empty space to add a band there: a cut at the far edges, a shelf near them, a
  bell anywhere else.
- **Drag** a band for frequency and threshold. Hold **shift** for fine moves.
- **Scroll** over a band to change its Q.
- **Alt-drag** a band to solo it while you sweep.
- **Double-click** a band to switch it off. Click empty space to deselect.
- **Hover** for the frequency, nearest note, input level and gain change at that point.

## Bands

Selecting a band, on the display or in the chip row below it, opens its panel.

- **Shape** — low cut, low shelf, bell, high shelf, high cut. The cuts do not filter the audio; they
  set where compression works at all.
- **Freq**, **Q** (a bell's bandwidth, a shelf's slope, how sharply a cut sets in), and
  **Threshold** (±48 dB relative to the main Threshold, so a band can reach quiet detail on its
  own).
- **Band dynamics** — **Ratio** is the band's own ratio (1:1 to 20:1), or **Main** to follow the
  main Ratio. **Attack/Release ×** (×0.25 to ×4) scales the main timing inside the band, **Range**
  is the most gain change the band allows, and **Mode** follows the header or makes the band
  downward, upward or off.
- **Solo** plays only the band's range. With Delta on, you hear only what that band removes.

## The rest

- **Mode** — Downward, Upward or Both.
- **Sidechain** — Int (the input drives it), Ext Match (the threshold follows the sidechain's
  spectrum) or Ext Duck (the sidechain drives the compression).
- **Compressor** — Threshold, Ratio (for both directions) and Knee.
- **Time** — Attack and Release: how fast compression sets in when a frequency gets louder, and
  how fast it lets go.
- **Output** — Mix and Output.
- **Resolution** — Fast, Balanced, Fine or Ultra: finer separates resonances more cleanly, at the
  cost of latency. Click to step forward, right-click to step back.
- **Delta** — hear only what the compression changes.
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
