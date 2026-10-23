# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic
Versioning](https://semver.org/spec/v2.0.0.html).

Everything below [0.1.0](#010---2026-09-06) is the history of
[Spectral Compressor](https://github.com/robbert-vdh/spectral-compressor), the
plugin this is forked from — kept for attribution, since the DSP core is
still largely its code.

## [0.2.0] - 2026-10-23

Bands with Pro-Q detail and a face as small as soothe's, in the graphite edition of the Sungam
design system, with controls named the way a classic compressor and EQ name them.

### Added

- Eight bands in place of the six nodes, each like a dynamic EQ band: a shape (low cut, low shelf,
  bell, high shelf, high cut), Freq, Q, a Threshold relative to the main one, and band dynamics:
  Ratio ×, Attack/Release ×, Range and Mode. Cuts set where compression works rather than filter
  the audio. The bands are resolved into per-bin arrays only when they change, and envelope
  followers now run at a per-bin speed.
- A global Mode: Downward, Upward or Both. Both directions take the Ratio control, and upward
  compression is capped at 24 dB.
- A floating band panel that follows the selected band, with a shape picker, the band's controls
  and Solo.
- Delta, to hear only what the compression changes, and Bypass, the host's bypass.
- Band Solo, from the panel or by alt-dragging a band.
- Factory and user presets, with a preset bar in the header. The preset name is saved with the
  session.
- Display gestures: double-click to create a band, shift-drag for fine moves, scroll for Q, a
  hover readout with frequency, note, input level and gain change.

### Changed

- The editor is now in the graphite (dark) edition of the Sungam design system, matching Beygla and
  Samplebrain: Menlo, square corners, hairline frames, and bar sliders in place of the knobs.
- Controls use classic compressor names: Depth is Threshold, Sharpness is Ratio, Selectivity is
  Knee, Trim is Output. The sidechain modes are Int, Ext Match and Ext Duck.
- The display draws a flat threshold line on its own scale instead of the sloped absolute
  threshold; pulling a band down compresses harder. The gain reduction shows as amber between the
  input and output spectra.
- One Resolution control (Fast, Balanced, Fine, Ultra) sets window size and overlap together.
- The window is 1200×645 and the footer is gone.

### Removed

- The Tilt, Bend and Center controls. Shelves and wide bells do the same job, visibly. The pink
  noise tilt is still applied, pivoting at 420 Hz.
- The six threshold nodes. Sessions saved with 0.1.0 load without them.
- The separate upwards ratio. It is hidden and kept only so old sessions still load.

## [0.1.0] - 2026-09-06

Ómhemill: a themed fork of Spectral Compressor with a custom Vizia editor in
the [Sungam](https://sungammagnus.com) plugin palette.

### Added

- Six draggable threshold-curve nodes (`ThresholdParams::nodes`), each with
  its own frequency, depth and width, evaluated as gaussian dents on top of
  the existing polynomial curve in `Curve::evaluate_ln`. Drag a node's
  handle directly on the spectrum display to move it; double-click to
  disable it.
- A custom rotary knob widget (`editor/knob.rs`) replacing the stock
  generic-UI sliders, built against nih-plug's `ParamWidgetBase` so
  automation and host modulation display work the same as any built-in
  widget.
- A redrawn spectrum display: log-frequency and decibel grid with axis
  labels, a legend, and the node handles described above.
- A standalone preview binary (`--features standalone`) for looking at the
  GUI without a host.

### Changed

- Rebranded: plugin name, VST3/CLAP IDs, window title.
- `Window Size` → `Resolution`, `Window Overlap` → `Overlap`; the four
  editor columns are now Detect / Shape / Time / Spectrum / Output rather
  than Globals / Threshold / Upwards / Downwards.
- Threshold mode display names shortened (`Pink`, `SC Match`, `SC Duck`) so
  they fit the mode control without overlapping.

## [0.4.3] - 2023-03-31

### Changed

- The default window size has been changed to 2048 since this offers a slightly
  better tradeoff between faster timings and more spectral precision. Existing
  instances are not affected.

### Fixed

- Fixed the soft-knee options in the sidechain matching mode. They previously
  didn't account for the changing compressor thresholds, which could result in
  unexpected loud volume spikes.
- The sidechain matching mode now caps the relative thresholds to behave more
  consistently with quiet inputs.

## [0.4.2] - 2023-03-22

### Changed

- Reworked the resetting behavior again to smoothly fade the timings in after a
  reset to allow the envelop followers to more gently settle in.

## [0.4.1] - 2023-03-22

### Fixed

- Fixed a regression from version 0.4.0 that caused the envelope followers to
  always be stuck at their fastest timings.

## [0.4.0] - 2023-03-22

### Added

- Added an analyzer that visualizes the target curve, the spectral envelope
  followers and gain reduction. The current version will be expanded a bit in a
  future with tooltips and labels to show more information.

### Changed

- The envelope followers reset in a smarter way after the plugin resumes from
  sleep or when the window size has changed. This avoids loud spikes in these
  situations when using extreme compression settings and slow timings.
- The default window overlap amount setting has changed to 16x. Existing patches
  are not affected.
- On Windows, clicking on the plugin's name no longer takes you to Spectral
  Compressor's home page. This is a temporary workaround for an issue with an
  underlying library.

## [0.3.0] - 2023-01-15

### Added

- Added the version number and a link to the GitHub page to the GUI to make it
  easier to determine which version you're using.

### Removed

- The DC filter option is gone. It was used to prevent upwards compression from
  amplifying DC and very low subbass signals in the original Spectral
  Compressor, but this iteration of Spectral Compressor doesn't need it and the
  feature only caused confusion.

### Changed

- The compression part of Spectral Compressor has been rewritten to have
  theoretically smoother and cleaner transfer curves and to be slightly more
  performant.
- The downwards hi-freq rolloff parameter now correctly scales the ratios.
  Previously the parameter didn't do much.
