# Ómhemill

*óm* (resonance) + *hemill* (brake) — the resonance brake.

A spectral resonance suppressor: a 16k-band per-bin compressor that can
suppress resonances, lift buried detail, or both at once, with a threshold
curve you can shape by ear rather than by node-dragging on an EQ.

This is a themed fork of Robbert van der Helm's
[Spectral Compressor](https://github.com/robbert-vdh/spectral-compressor)
(GPL-3.0-or-later) — the DSP core (STFT engine, per-bin upwards/downwards
compressor bank, threshold curve math) is his, largely unmodified. What
changed here is the branding, the Vizia editor's colour theme (recoloured to
match [Sungam](https://sungammagnus.com)'s cream/ink plugin palette — the
same one used by Colacut, Skyjað, and the rest of the line), and a handful of
parameter display names (`Window Size` → `Resolution`, `Window Overlap` →
`Overlap`, column titles `Threshold`/`Upwards`/`Downwards` → `Curve`/`Lift`/
`Suppress`) to match the language used in the
[web prototype](https://sungammagnus.com) and concept doc this was built
alongside. See `COPYING` for the full license text and the original
copyright headers left intact in the source files.

## Modes

- **Suppress** — downwards only. Tames resonances, harshness, sibilance.
- **Lift** — upwards only. Pulls buried detail up toward the curve.
- **Both** — the 16k-band OTT. Everything becomes pink noise if you try
  hard enough.
- Internal / sidechain-match / sidechain-compress threshold modes (in the
  **Curve** column) let the threshold track the plugin's own input or a
  second signal.

## Building

```shell
cargo xtask bundle omhemill --release
```

This produces `target/bundled/Omhemill.vst3` and `target/bundled/Omhemill.clap`.

### Looking at the GUI without a host

There is a windowed preview build. It is behind a feature flag because
nih-plug's standalone wrapper links against `libjack`, and the plugin itself
must not pick that dependency up or hosts will refuse to load it:

```shell
RUSTFLAGS="-L /usr/local/opt/jack/lib" cargo run --release --bin omhemill_standalone --features standalone -- --backend dummy
```

The `-L` is needed on macOS because Homebrew leaves a stale
`/usr/local/lib/libjack.dylib` symlink pointing at a framework that is not
installed; the real library lives in the Cellar.

## Related

- [Concept doc and web prototype](../..) — the design rationale, parameter
  mapping, and a real-time Web Audio implementation of the same idea.
