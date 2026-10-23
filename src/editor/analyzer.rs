// Spectral Compressor: an FFT based compressor
// Copyright (C) 2021-2024 Robbert van der Helm
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

use atomic_float::AtomicF32;
use nih_plug::prelude::Enum;
use nih_plug_vizia::vizia::prelude::*;
use nih_plug_vizia::vizia::vg;
use nih_plug_vizia::widgets::param_base::ParamWidgetBase;
use nih_plug_vizia::widgets::util::ModifiersExt;
use std::cell::Cell;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use super::{BandSelection, Data, SoloEvent, BAND_COLORS};
use crate::analyzer::AnalyzerData;
use crate::curve::{octaves_from_q, BandShape, Curve, NUM_BANDS};
use crate::OmhemillParams;

/// The threshold line is the thing you are actually shaping, so it is drawn in full ink.
const THRESHOLD_LINE_COLOR: vg::Color = vg::Color::rgbaf(0.925, 0.910, 0.875, 1.0);
/// The threshold line shows the bands' thresholds relative to the main one, on its own scale
/// centred on the display like an EQ's gain axis: this many decibels reach
/// [`THRESHOLD_HALF_HEIGHT`] of the way to the edge. Down is a lower threshold, so more
/// compression, the way pulling an EQ band down cuts.
const THRESHOLD_RANGE_DB: f32 = 18.0;
const THRESHOLD_HALF_HEIGHT: f32 = 0.45;
const NODE_DISABLED_COLOR: vg::Color = vg::Color::rgbaf(0.925, 0.910, 0.875, 0.38);
/// The stage the spectrum sits on, used to punch the number out of a filled handle.
const STAGE_COLOR: vg::Color = vg::Color::rgbaf(0.125, 0.122, 0.118, 1.0);
/// The panel colour, behind the hover readout.
const PAPER_COLOR: vg::Color = vg::Color::rgbaf(0.180, 0.176, 0.169, 1.0);
/// Half the side of a (square) band handle.
const NODE_RADIUS: f32 = 10.0;

/// Amber is reserved for live state: what the brake is taking away right now...
const REDUCTION_COLOR: vg::Color = vg::Color::rgbaf(0.851, 0.639, 0.161, 0.75);
/// ...and, a shade lighter, what it is lifting.
const LIFT_COLOR: vg::Color = vg::Color::rgbaf(0.941, 0.812, 0.478, 0.55);

/// Shift-dragging a band moves it this much slower than the pointer.
const GRANULAR_DRAG_MULTIPLIER: f32 = 0.1;

// We'll show the bins from 30 Hz (to your chest) to 22 kHz, scaled logarithmically
#[allow(unused)]
const FREQ_RANGE_START_HZ: f32 = 30.0;
#[allow(unused)]
const FREQ_RANGE_END_HZ: f32 = 22_000.0;
const LN_FREQ_RANGE_START_HZ: f32 = 3.4011974; // 30.0f32.ln();
const LN_FREQ_RANGE_END_HZ: f32 = 9.998797; // 22_000.0f32.ln();
const LN_FREQ_RANGE: f32 = LN_FREQ_RANGE_END_HZ - LN_FREQ_RANGE_START_HZ;

const NOTE_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

/// The parameter handles for a single draggable band, so the analyzer can move it around while
/// still producing proper automation gestures.
struct BandHandles {
    enabled: ParamWidgetBase,
    shape: ParamWidgetBase,
    frequency: ParamWidgetBase,
    threshold_db: ParamWidgetBase,
    q: ParamWidgetBase,
}

/// A band being dragged. The pointer's movement is accumulated into a virtual position so holding
/// shift slows the band down without making it jump.
#[derive(Debug, Clone, Copy)]
struct BandDrag {
    idx: usize,
    /// Set when the drag soloed the band (alt-drag), to the solo to go back to afterwards.
    restore_solo: Option<Option<usize>>,
    last_x: f32,
    last_y: f32,
    virtual_x: f32,
    virtual_y: f32,
}

/// The spectrum display: the input spectrum with what the brake removes (or lifts) in amber, the
/// threshold line, and the band handles. Bands are dragged, scrolled and created right here.
pub struct Analyzer {
    analyzer_data: Arc<Mutex<triple_buffer::Output<AnalyzerData>>>,
    sample_rate: Arc<AtomicF32>,
    /// Read during drawing to place the band handles.
    params: Arc<OmhemillParams>,
    /// Used to actually move a band when it gets dragged.
    bands: Vec<BandHandles>,
    drag: Option<BandDrag>,
    /// Where the pointer is, for the readout, while it is over the display.
    hover: Option<(f32, f32)>,
    /// Scroll wheel movement that has not yet added up to a whole step.
    scrolled_lines: f32,
    /// Lazily registered with the canvas the first time we draw text on it.
    font: Cell<Option<vg::FontId>>,
}

impl Analyzer {
    /// Creates a new [`Analyzer`].
    pub fn new<LAnalyzerData, LRate, LParams>(
        cx: &mut Context,
        analyzer_data: LAnalyzerData,
        sample_rate: LRate,
        params: LParams,
    ) -> Handle<'_, Self>
    where
        LAnalyzerData: Lens<Target = Arc<Mutex<triple_buffer::Output<AnalyzerData>>>>,
        LRate: Lens<Target = Arc<AtomicF32>>,
        LParams: Lens<Target = Arc<OmhemillParams>> + Clone,
    {
        let bands = (0..NUM_BANDS)
            .map(|idx| BandHandles {
                enabled: ParamWidgetBase::new(cx, params.clone(), move |p| {
                    &p.threshold.bands[idx].enabled
                }),
                shape: ParamWidgetBase::new(cx, params.clone(), move |p| {
                    &p.threshold.bands[idx].shape
                }),
                frequency: ParamWidgetBase::new(cx, params.clone(), move |p| {
                    &p.threshold.bands[idx].frequency
                }),
                threshold_db: ParamWidgetBase::new(cx, params.clone(), move |p| {
                    &p.threshold.bands[idx].threshold_db
                }),
                q: ParamWidgetBase::new(cx, params.clone(), move |p| {
                    &p.threshold.bands[idx].q
                }),
            })
            .collect();

        Self {
            analyzer_data: analyzer_data.get(cx),
            sample_rate: sample_rate.get(cx),
            params: params.get(cx),
            bands,
            drag: None,
            hover: None,
            scrolled_lines: 0.0,
            font: Cell::new(None),
        }
        .build(
            cx,
            // This is an otherwise empty element only used for custom drawing
            |_cx| (),
        )
    }

    /// The horizontal position of a frequency within `bounds`, as a `[0, 1]` fraction.
    pub fn freq_to_t(freq: f32) -> f32 {
        (freq.ln() - LN_FREQ_RANGE_START_HZ) / LN_FREQ_RANGE
    }

    /// The inverse of [`Self::freq_to_t()`].
    fn t_to_freq(t: f32) -> f32 {
        (LN_FREQ_RANGE_START_HZ + (LN_FREQ_RANGE * t)).exp()
    }

    /// Where each band handle sits, in physical coordinates, along with whether it is enabled
    /// and whether it is a cut.
    fn band_positions(&self, bounds: BoundingBox) -> Vec<(f32, f32, bool, bool)> {
        let threshold = &self.params.threshold;
        let curve_params = threshold.curve_params();
        let curve = Curve::new(&curve_params);

        (0..NUM_BANDS)
            .map(|idx| {
                let band = &threshold.bands[idx];
                let freq = band.frequency.value();
                let is_cut = band.shape.value().is_cut();
                let x = bounds.x + bounds.w * Self::freq_to_t(freq).clamp(0.0, 1.0);
                // A cut has no threshold of its own, so it sits on the zero line
                let threshold_db = if is_cut {
                    0.0
                } else {
                    -curve.sensitivity_ln(freq.ln())
                };
                let y = threshold_to_y(bounds, threshold_db);

                (x, y, band.enabled.value(), is_cut)
            })
            .collect()
    }

    /// The band under the pointer, if any.
    fn band_at(&self, bounds: BoundingBox, mouse_x: f32, mouse_y: f32, scale: f32) -> Option<usize> {
        let grab_radius = NODE_RADIUS * scale * 1.6;

        self.band_positions(bounds)
            .into_iter()
            .enumerate()
            .map(|(idx, (x, y, _, _))| {
                let dx = x - mouse_x;
                let dy = y - mouse_y;
                (idx, (dx * dx) + (dy * dy))
            })
            .filter(|(_, distance_sq)| *distance_sq <= grab_radius * grab_radius)
            .min_by(|(_, a), (_, b)| a.total_cmp(b))
            .map(|(idx, _)| idx)
    }

    /// The other bands' summed sensitivity at `freq`, with `skip_idx` taken out, so a drag can be
    /// translated back into a threshold for that band.
    fn sensitivity_without_band(&self, freq: f32, skip_idx: usize) -> f32 {
        let mut curve_params = self.params.threshold.curve_params();
        curve_params.bands[skip_idx].enabled = false;

        Curve::new(&curve_params).sensitivity_ln(freq.ln())
    }

    /// The frequency range a band covers, for shading the selected band and metering it. Cuts
    /// cover nothing: what they fence off is already dimmed.
    fn band_range_hz(&self, idx: usize) -> Option<(f32, f32)> {
        let band = &self.params.threshold.bands[idx];
        let freq = band.frequency.value();
        let half_width = octaves_from_q(band.q.value()) / 2.0;
        match band.shape.value() {
            BandShape::Bell => Some((freq * 2.0f32.powf(-half_width), freq * 2.0f32.powf(half_width))),
            BandShape::LowShelf => Some((FREQ_RANGE_START_HZ, freq)),
            BandShape::HighShelf => Some((freq, FREQ_RANGE_END_HZ)),
            BandShape::LowCut | BandShape::HighCut => None,
        }
    }

    /// Move a band to a point on the display: frequency from `x`, and from `y` the threshold that
    /// puts the line there, given what the other bands already do. A cut only moves sideways.
    fn move_band_to(&self, cx: &mut EventContext, bounds: BoundingBox, idx: usize, x: f32, y: f32) {
        let t = ((x - bounds.x) / bounds.w).clamp(0.0, 1.0);
        let freq = Self::t_to_freq(t);
        let band = &self.bands[idx];
        band.frequency
            .set_normalized_value(cx, band.frequency.preview_normalized(freq));

        if !self.params.threshold.bands[idx].shape.value().is_cut() {
            // The line shows minus the summed sensitivity, so the other bands' part is added back
            let target_db = y_to_threshold(bounds, y);
            let threshold_db = target_db + self.sensitivity_without_band(freq, idx);
            band.threshold_db
                .set_normalized_value(cx, band.threshold_db.preview_normalized(threshold_db));
        }
    }

    /// Switch on the first free band at the pointer, with a shape that suits where it lands: cuts
    /// at the very edges, shelves near them, bells everywhere else.
    fn create_band_at(&self, cx: &mut EventContext, bounds: BoundingBox, x: f32, y: f32) {
        let Some(idx) = (0..NUM_BANDS).find(|idx| !self.params.threshold.bands[*idx].enabled.value())
        else {
            return;
        };

        let t = ((x - bounds.x) / bounds.w).clamp(0.0, 1.0);
        let freq = Self::t_to_freq(t);
        let shape = if t < 0.04 {
            BandShape::LowCut
        } else if t > 0.97 {
            BandShape::HighCut
        } else if freq < 100.0 {
            BandShape::LowShelf
        } else if freq > 9_000.0 {
            BandShape::HighShelf
        } else {
            BandShape::Bell
        };

        let band = &self.bands[idx];
        for param in [&band.enabled, &band.shape, &band.frequency, &band.threshold_db] {
            param.begin_set_parameter(cx);
        }
        band.enabled.set_normalized_value(cx, 1.0);
        band.shape
            .set_normalized_value(cx, band.shape.preview_normalized(shape.to_index() as f32));
        // The shape has to be in place before the threshold is worked out
        self.move_band_to(cx, bounds, idx, x, y);
        for param in [&band.enabled, &band.shape, &band.frequency, &band.threshold_db] {
            param.end_set_parameter(cx);
        }

        cx.emit(BandSelection::Select(Some(idx)));
    }
}

impl View for Analyzer {
    fn element(&self) -> Option<&'static str> {
        Some("analyzer")
    }

    fn event(&mut self, cx: &mut EventContext, event: &mut Event) {
        let bounds = cx.bounds();

        event.map(|window_event, meta| match window_event {
            WindowEvent::MouseDown(MouseButton::Left) => {
                let (mouse_x, mouse_y) = (cx.mouse().cursorx, cx.mouse().cursory);
                match self.band_at(bounds, mouse_x, mouse_y, cx.scale_factor()) {
                    Some(idx) => {
                        let band = &self.bands[idx];
                        cx.emit(BandSelection::Select(Some(idx)));

                        // Grabbing a disabled band switches it on, which is what you meant by
                        // grabbing it
                        if !self.params.threshold.bands[idx].enabled.value() {
                            band.enabled.begin_set_parameter(cx);
                            band.enabled.set_normalized_value(cx, 1.0);
                            band.enabled.end_set_parameter(cx);
                        }

                        // Alt-dragging solos the band while you sweep it
                        let restore_solo = if cx.modifiers().alt() {
                            let previous = Data::solo_band.get(cx);
                            cx.emit(SoloEvent::Set(Some(idx)));
                            Some(previous)
                        } else {
                            None
                        };

                        self.drag = Some(BandDrag {
                            idx,
                            restore_solo,
                            last_x: mouse_x,
                            last_y: mouse_y,
                            virtual_x: mouse_x,
                            virtual_y: mouse_y,
                        });
                        cx.capture();
                        cx.set_active(true);

                        band.frequency.begin_set_parameter(cx);
                        band.threshold_db.begin_set_parameter(cx);
                    }
                    // Clicking the empty display lets go of the selection
                    None => cx.emit(BandSelection::Select(None)),
                }

                meta.consume();
            }
            WindowEvent::MouseDoubleClick(MouseButton::Left) => {
                let (mouse_x, mouse_y) = (cx.mouse().cursorx, cx.mouse().cursory);
                match self.band_at(bounds, mouse_x, mouse_y, cx.scale_factor()) {
                    // A double click on a band switches it off again
                    Some(idx) => {
                        if self.params.threshold.bands[idx].enabled.value() {
                            let band = &self.bands[idx];
                            band.enabled.begin_set_parameter(cx);
                            band.enabled.set_normalized_value(cx, 0.0);
                            band.enabled.end_set_parameter(cx);
                        }
                    }
                    // And one on empty space makes a new band there
                    None => self.create_band_at(cx, bounds, mouse_x, mouse_y),
                }

                meta.consume();
            }
            WindowEvent::MouseUp(MouseButton::Left) => {
                if let Some(drag) = self.drag.take() {
                    if let Some(previous) = drag.restore_solo {
                        cx.emit(SoloEvent::Set(previous));
                    }

                    let band = &self.bands[drag.idx];
                    band.frequency.end_set_parameter(cx);
                    band.threshold_db.end_set_parameter(cx);

                    cx.release();
                    cx.set_active(false);

                    meta.consume();
                }
            }
            WindowEvent::MouseMove(x, y) => {
                if let Some(mut drag) = self.drag {
                    if bounds.w <= 0.0 || bounds.h <= 0.0 {
                        return;
                    }

                    let multiplier = if cx.modifiers().shift() {
                        GRANULAR_DRAG_MULTIPLIER
                    } else {
                        1.0
                    };
                    drag.virtual_x += (*x - drag.last_x) * multiplier;
                    drag.virtual_y += (*y - drag.last_y) * multiplier;
                    drag.last_x = *x;
                    drag.last_y = *y;
                    self.drag = Some(drag);

                    self.move_band_to(cx, bounds, drag.idx, drag.virtual_x, drag.virtual_y);
                }

                self.hover = Some((*x, *y));
                cx.needs_redraw();
            }
            WindowEvent::MouseLeave => {
                self.hover = None;
                cx.needs_redraw();
            }
            WindowEvent::MouseScroll(_scroll_x, scroll_y) => {
                // Scrolling over a band changes its Q: up is narrower
                let (mouse_x, mouse_y) = (cx.mouse().cursorx, cx.mouse().cursory);
                let hovered = self
                    .drag
                    .map(|drag| drag.idx)
                    .or_else(|| self.band_at(bounds, mouse_x, mouse_y, cx.scale_factor()));
                if let Some(idx) = hovered {
                    self.scrolled_lines += scroll_y;
                    if self.scrolled_lines.abs() >= 1.0 {
                        let width = &self.bands[idx].q;
                        let use_finer_steps = cx.modifiers().shift();
                        let mut value = width.unmodulated_normalized_value();
                        while self.scrolled_lines >= 1.0 {
                            value = width.next_normalized_step(value, use_finer_steps);
                            self.scrolled_lines -= 1.0;
                        }
                        while self.scrolled_lines <= -1.0 {
                            value = width.previous_normalized_step(value, use_finer_steps);
                            self.scrolled_lines += 1.0;
                        }

                        width.begin_set_parameter(cx);
                        width.set_normalized_value(cx, value);
                        width.end_set_parameter(cx);
                    }

                    meta.consume();
                }
            }
            _ => {}
        });
    }

    fn draw(&self, cx: &mut DrawContext, canvas: &mut Canvas) {
        let bounds = cx.bounds();
        if bounds.w == 0.0 || bounds.h == 0.0 {
            return;
        }

        let selected = Data::selected_band.get(cx);

        {
            // The analyzer data is pulled directly from the spectral `CompressorBank`
            let mut analyzer_data = self.analyzer_data.lock().unwrap();
            let analyzer_data = analyzer_data.read();
            let nyquist = self.sample_rate.load(Ordering::Relaxed) / 2.0;

            self.draw_grid(cx, canvas);
            if let Some(idx) = selected {
                self.draw_band_region(cx, canvas, idx);
            }
            draw_spectrum(cx, canvas, analyzer_data, nyquist);
            self.draw_threshold_line(cx, canvas);
            self.draw_bands(cx, canvas, selected, analyzer_data, nyquist);
            self.draw_legend(cx, canvas);
            self.draw_hover_readout(cx, canvas, analyzer_data, nyquist);
        }

        // Draw the border last
        let border_width = cx.border_width();
        let border_color: vg::Color = cx.border_color().into();

        let mut path = vg::Path::new();
        {
            let x = bounds.x + border_width / 2.0;
            let y = bounds.y + border_width / 2.0;
            let w = bounds.w - border_width;
            let h = bounds.h - border_width;
            path.move_to(x, y);
            path.line_to(x, y + h);
            path.line_to(x + w, y + h);
            path.line_to(x + w, y);
            path.close();
        }

        let paint = vg::Paint::color(border_color).with_line_width(border_width);
        canvas.stroke_path(&path, &paint);
    }
}

/// Compute an unclamped value based on a decibel value -80 and is mapped to 0, +20 is mapped to 1,
/// and all other values are linearly interpolated from there
#[inline]
fn db_to_unclamped_t(db_value: f32) -> f32 {
    (db_value + 80.0) / 100.0
}

/// Where a band threshold sits vertically within `bounds`.
fn threshold_to_y(bounds: BoundingBox, threshold_db: f32) -> f32 {
    let t = (threshold_db / THRESHOLD_RANGE_DB).clamp(-1.0, 1.0) * THRESHOLD_HALF_HEIGHT;
    bounds.y + bounds.h * (0.5 - t)
}

/// The inverse of [`threshold_to_y()`].
fn y_to_threshold(bounds: BoundingBox, y: f32) -> f32 {
    let t = 0.5 - ((y - bounds.y) / bounds.h);
    (t / THRESHOLD_HALF_HEIGHT) * THRESHOLD_RANGE_DB
}

/// A band's own colour. See [`BAND_COLORS`].
fn band_color(idx: usize, alpha: f32) -> vg::Color {
    let (r, g, b) = BAND_COLORS[idx % BAND_COLORS.len()];
    vg::Color::rgba(r, g, b, (alpha * 255.0).round() as u8)
}

/// A frequency as the readout writes it.
fn format_frequency(freq: f32) -> String {
    if freq < 1_000.0 {
        format!("{freq:.0} Hz")
    } else {
        format!("{:.2} kHz", freq / 1_000.0)
    }
}

/// The nearest note to a frequency, with how many cents it is off, as in `E7 +12`.
fn format_note(freq: f32) -> String {
    let midi = 69.0 + 12.0 * (freq / 440.0).log2();
    let nearest = midi.round();
    let cents = ((midi - nearest) * 100.0).round() as i32;
    let note = nearest as i32;
    let name = NOTE_NAMES[note.rem_euclid(12) as usize];
    let octave = note.div_euclid(12) - 1;

    format!("{name}{octave} {cents:+}")
}

impl Analyzer {
    /// Register (once) and return the font used for the axis labels.
    fn font(&self, canvas: &mut Canvas) -> Option<vg::FontId> {
        if let Some(font) = self.font.get() {
            return Some(font);
        }

        // The same monospace face the stylesheet asks for. It is a system font, so fall back to
        // the bundled Noto Sans where it is not installed.
        let font = canvas
            .add_font("/System/Library/Fonts/Menlo.ttc")
            .or_else(|_| canvas.add_font_mem(nih_plug_vizia::assets::fonts::NOTO_SANS_REGULAR))
            .ok()?;
        self.font.set(Some(font));

        Some(font)
    }

    /// A text paint in the display's font.
    fn text_paint(&self, canvas: &mut Canvas, color: vg::Color, size: f32) -> Option<vg::Paint> {
        let font = self.font(canvas)?;
        let mut paint = vg::Paint::color(color);
        paint.set_font(&[font]);
        paint.set_font_size(size);

        Some(paint)
    }

    /// A light reference grid with frequency and decibel labels, drawn behind everything else.
    fn draw_grid(&self, cx: &mut DrawContext, canvas: &mut Canvas) {
        let bounds = cx.bounds();
        let scale = cx.scale_factor();

        let ink: vg::Color = cx.border_color().into();
        let grid_paint = vg::Paint::color(vg::Color::rgbaf(ink.r, ink.g, ink.b, 0.07))
            .with_line_width(scale);
        let label_color = vg::Color::rgbaf(ink.r, ink.g, ink.b, 0.45);

        for (freq, label) in [
            (30.0f32, "30"),
            (100.0, "100"),
            (300.0, "300"),
            (1_000.0, "1k"),
            (3_000.0, "3k"),
            (10_000.0, "10k"),
        ] {
            let t = Self::freq_to_t(freq);
            if !(0.0..=1.0).contains(&t) {
                continue;
            }

            let x = bounds.x + bounds.w * t;
            let mut path = vg::Path::new();
            path.move_to(x, bounds.y);
            path.line_to(x, bounds.y + bounds.h);
            canvas.stroke_path(&path, &grid_paint);

            if let Some(mut paint) = self.text_paint(canvas, label_color, 10.0 * scale) {
                paint.set_text_align(vg::Align::Center);
                paint.set_text_baseline(vg::Baseline::Bottom);
                let _ = canvas.fill_text(x, bounds.y + bounds.h - (4.0 * scale), label, &paint);
            }
        }

        for db in [-12.0f32, -24.0, -36.0, -48.0, -60.0, -72.0] {
            let y = bounds.y + bounds.h * (1.0 - db_to_unclamped_t(db));
            if y < bounds.y || y > bounds.y + bounds.h {
                continue;
            }

            let mut path = vg::Path::new();
            path.move_to(bounds.x, y);
            path.line_to(bounds.x + bounds.w, y);
            canvas.stroke_path(&path, &grid_paint);

            if let Some(mut paint) = self.text_paint(canvas, label_color, 10.0 * scale) {
                paint.set_text_align(vg::Align::Right);
                paint.set_text_baseline(vg::Baseline::Middle);
                let _ = canvas.fill_text(
                    bounds.x + bounds.w - (6.0 * scale),
                    y,
                    format!("{db:.0} dB"),
                    &paint,
                );
            }
        }

        // The threshold scale on the left, and its zero line dashed across the display
        let mut zero_line = vg::Path::new();
        let zero_y = threshold_to_y(bounds, 0.0).round() + 0.5;
        let (dash, gap) = (3.0 * scale, 4.0 * scale);
        let mut x = bounds.x;
        while x < bounds.x + bounds.w {
            zero_line.move_to(x, zero_y);
            zero_line.line_to((x + dash).min(bounds.x + bounds.w), zero_y);
            x += dash + gap;
        }
        canvas.stroke_path(
            &zero_line,
            &vg::Paint::color(vg::Color::rgbaf(ink.r, ink.g, ink.b, 0.22)).with_line_width(scale),
        );

        for threshold_db in [12.0f32, 0.0, -12.0] {
            if let Some(mut paint) = self.text_paint(canvas, label_color, 10.0 * scale) {
                paint.set_text_align(vg::Align::Left);
                paint.set_text_baseline(vg::Baseline::Middle);
                let _ = canvas.fill_text(
                    bounds.x + (6.0 * scale),
                    threshold_to_y(bounds, threshold_db),
                    format!("{threshold_db:+.0}").replace("+0", "0"),
                    &paint,
                );
            }
        }
    }

    /// A faint wash in the band's colour over the range the selected band covers.
    fn draw_band_region(&self, cx: &mut DrawContext, canvas: &mut Canvas, idx: usize) {
        if !self.params.threshold.bands[idx].enabled.value() {
            return;
        }
        let Some((low_hz, high_hz)) = self.band_range_hz(idx) else {
            return;
        };

        let bounds = cx.bounds();
        let left = bounds.x + bounds.w * Self::freq_to_t(low_hz).clamp(0.0, 1.0);
        let right = bounds.x + bounds.w * Self::freq_to_t(high_hz).clamp(0.0, 1.0);

        let mut region = vg::Path::new();
        region.rect(left, bounds.y, right - left, bounds.h);
        canvas.fill_path(
            &region,
            &vg::Paint::color(band_color(idx, 0.09)),
        );
    }

    /// The threshold line: flat at zero until a band bends it, like an EQ's response curve.
    /// Where the cuts (or a band set to Off) fence the brake off, the stage is dimmed and the line
    /// drops out. This reads the parameters directly so it follows a drag without waiting for the
    /// audio thread.
    fn draw_threshold_line(&self, cx: &mut DrawContext, canvas: &mut Canvas) {
        let bounds = cx.bounds();
        let scale = cx.scale_factor();
        let curve_params = self.params.threshold.curve_params();
        let curve = Curve::new(&curve_params);

        let num_points = 200.min(bounds.w.ceil() as usize).max(2);
        let mut line = vg::Path::new();
        let mut fenced = vg::Path::new();
        let mut pen_down = false;
        let step_w = bounds.w / (num_points - 1) as f32;
        for i in 0..num_points {
            let x_t = i as f32 / (num_points - 1) as f32;
            let ln_freq = LN_FREQ_RANGE_START_HZ + (LN_FREQ_RANGE * x_t);
            let x = bounds.x + (bounds.w * x_t);

            let shape = curve.shape_ln(ln_freq);
            if shape.down_weight.max(shape.up_weight) < 0.5 {
                fenced.rect(x - step_w / 2.0, bounds.y, step_w, bounds.h);
                pen_down = false;
                continue;
            }

            let y = threshold_to_y(bounds, -shape.sensitivity_db);
            if pen_down {
                line.line_to(x, y);
            } else {
                line.move_to(x, y);
                pen_down = true;
            }
        }

        canvas.scissor(bounds.x, bounds.y, bounds.w, bounds.h);
        canvas.fill_path(
            &fenced,
            &vg::Paint::color(vg::Color::rgbaf(0.0, 0.0, 0.0, 0.28)).with_anti_alias(false),
        );
        canvas.stroke_path(
            &line,
            &vg::Paint::color(THRESHOLD_LINE_COLOR).with_line_width(2.0 * scale),
        );
        canvas.reset_scissor();
    }

    /// A key for the three things drawn on the graph, in the top right corner.
    fn draw_legend(&self, cx: &mut DrawContext, canvas: &mut Canvas) {
        let bounds = cx.bounds();
        let scale = cx.scale_factor();
        let spectrum_color: vg::Color = cx.font_color().into();
        let ink: vg::Color = cx.border_color().into();

        let Some(mut paint) = self.text_paint(
            canvas,
            vg::Color::rgbaf(ink.r, ink.g, ink.b, 0.55),
            9.0 * scale,
        ) else {
            return;
        };
        paint.set_text_align(vg::Align::Right);
        paint.set_text_baseline(vg::Baseline::Middle);

        let entries = [
            ("REDUCTION", vg::Color::rgbaf(0.851, 0.639, 0.161, 1.0)),
            ("THRESHOLD", THRESHOLD_LINE_COLOR),
            ("SPECTRUM", spectrum_color),
        ];

        let y = bounds.y + (14.0 * scale);
        let mut right = bounds.x + bounds.w - (12.0 * scale);
        for (label, color) in entries {
            let text_width = canvas
                .fill_text(right, y, label, &paint)
                .map(|metrics| metrics.width())
                .unwrap_or(50.0 * scale);

            // A short dash in the thing's own colour, just left of its label
            let dash_right = right - text_width - (6.0 * scale);
            let mut dash = vg::Path::new();
            dash.move_to(dash_right - (12.0 * scale), y);
            dash.line_to(dash_right, y);
            canvas.stroke_path(&dash, &vg::Paint::color(color).with_line_width(2.0 * scale));

            right = dash_right - (12.0 * scale) - (14.0 * scale);
        }
    }

    /// The numbered band handles on the threshold line, each in its own colour. The selected band
    /// is filled, the others are outlined, and disabled ones are grey. Under each enabled band hangs an amber bar for
    /// how much that band is braking right now.
    fn draw_bands(
        &self,
        cx: &mut DrawContext,
        canvas: &mut Canvas,
        selected: Option<usize>,
        analyzer_data: &AnalyzerData,
        nyquist_hz: f32,
    ) {
        let bounds = cx.bounds();
        let scale = cx.scale_factor();
        let radius = NODE_RADIUS * scale;

        canvas.scissor(bounds.x, bounds.y, bounds.w, bounds.h);

        for (idx, (x, y, enabled, _is_cut)) in self.band_positions(bounds).into_iter().enumerate() {
            let color = if enabled {
                band_color(idx, 1.0)
            } else {
                NODE_DISABLED_COLOR
            };
            let filled = enabled && selected == Some(idx);

            if enabled {
                if let Some((low_hz, high_hz)) = self.band_range_hz(idx) {
                    let braking_db = peak_gain_change_db(analyzer_data, nyquist_hz, low_hz, high_hz);
                    if braking_db >= 0.2 {
                        let mut meter = vg::Path::new();
                        meter.rect(
                            x - 2.0 * scale,
                            y + radius + 3.0 * scale,
                            4.0 * scale,
                            (braking_db * 4.0).min(80.0) * scale,
                        );
                        canvas.fill_path(&meter, &vg::Paint::color(REDUCTION_COLOR));
                    }
                }
            }

            // Square, like every other corner in the system
            let mut handle = vg::Path::new();
            handle.rect(x - radius, y - radius, radius * 2.0, radius * 2.0);
            let fill = if filled { color } else { STAGE_COLOR };
            canvas.fill_path(&handle, &vg::Paint::color(fill));
            canvas.stroke_path(&handle, &vg::Paint::color(color).with_line_width(1.4 * scale));

            let text_color = if filled { STAGE_COLOR } else { color };
            if let Some(mut paint) = self.text_paint(canvas, text_color, 11.0 * scale) {
                paint.set_text_align(vg::Align::Center);
                paint.set_text_baseline(vg::Baseline::Middle);
                let _ = canvas.fill_text(x, y, format!("{}", idx + 1), &paint);
            }
        }

        canvas.reset_scissor();
    }

    /// Under the pointer: a dotted line and a readout of the frequency, the nearest note, the
    /// input level and what the brake is doing there.
    fn draw_hover_readout(
        &self,
        cx: &mut DrawContext,
        canvas: &mut Canvas,
        analyzer_data: &AnalyzerData,
        nyquist_hz: f32,
    ) {
        let Some((mouse_x, _mouse_y)) = self.hover else {
            return;
        };
        let bounds = cx.bounds();
        if mouse_x < bounds.x || mouse_x > bounds.x + bounds.w || self.drag.is_some() {
            return;
        }
        let scale = cx.scale_factor();
        let ink: vg::Color = cx.border_color().into();

        let freq = Self::t_to_freq((mouse_x - bounds.x) / bounds.w);
        let (input_db, gain_change_db) = if analyzer_data.num_bins > 1 && nyquist_hz > 0.0 {
            let bin_idx = ((freq / nyquist_hz) * analyzer_data.num_bins as f32).round() as usize;
            let bin_idx = bin_idx.clamp(1, analyzer_data.num_bins - 1);
            (
                nih_plug::util::gain_to_db(analyzer_data.envelope_followers[bin_idx]),
                analyzer_data.gain_difference_db[bin_idx],
            )
        } else {
            (f32::NEG_INFINITY, 0.0)
        };

        // The dotted line
        let mut line = vg::Path::new();
        let mut y = bounds.y;
        while y < bounds.y + bounds.h {
            line.move_to(mouse_x.round() + 0.5, y);
            line.line_to(mouse_x.round() + 0.5, (y + scale).min(bounds.y + bounds.h));
            y += 4.0 * scale;
        }
        canvas.stroke_path(
            &line,
            &vg::Paint::color(vg::Color::rgbaf(ink.r, ink.g, ink.b, 0.38)).with_line_width(scale),
        );

        // The readout, kept inside the display
        let input = if input_db.is_finite() && input_db > -100.0 {
            format!("in {input_db:.1} dB")
        } else {
            String::from("in -inf")
        };
        let parts = [
            (format_frequency(freq), vg::Color::rgbaf(ink.r, ink.g, ink.b, 1.0)),
            (format_note(freq), vg::Color::rgbaf(ink.r, ink.g, ink.b, 0.55)),
            (input, vg::Color::rgbaf(ink.r, ink.g, ink.b, 0.55)),
            (format!("{gain_change_db:+.1} dB"), vg::Color::rgbaf(0.851, 0.639, 0.161, 1.0)),
        ];
        let Some(mut paint) = self.text_paint(canvas, PAPER_COLOR, 10.5 * scale) else {
            return;
        };
        paint.set_text_align(vg::Align::Left);
        paint.set_text_baseline(vg::Baseline::Middle);

        let gap = 10.0 * scale;
        let padding = 8.0 * scale;
        let widths: Vec<f32> = parts
            .iter()
            .map(|(text, _)| {
                canvas
                    .measure_text(0.0, 0.0, text, &paint)
                    .map(|metrics| metrics.width())
                    .unwrap_or(0.0)
            })
            .collect();
        let box_w = widths.iter().sum::<f32>() + gap * (parts.len() - 1) as f32 + padding * 2.0;
        let box_h = 20.0 * scale;
        let box_x = (mouse_x + 10.0 * scale).min(bounds.x + bounds.w - box_w - 4.0 * scale);
        let box_y = bounds.y + 28.0 * scale;

        let mut readout = vg::Path::new();
        readout.rect(box_x, box_y, box_w, box_h);
        canvas.fill_path(&readout, &vg::Paint::color(PAPER_COLOR));
        canvas.stroke_path(
            &readout,
            &vg::Paint::color(vg::Color::rgbaf(ink.r, ink.g, ink.b, 0.22)).with_line_width(scale),
        );

        let mut text_x = box_x + padding;
        for ((text, color), width) in parts.iter().zip(widths) {
            paint.set_color(*color);
            let _ = canvas.fill_text(text_x, box_y + box_h / 2.0, text, &paint);
            text_x += width + gap;
        }
    }
}

/// The largest gain change, in decibels and either way, across the bins between two frequencies.
fn peak_gain_change_db(
    analyzer_data: &AnalyzerData,
    nyquist_hz: f32,
    low_hz: f32,
    high_hz: f32,
) -> f32 {
    if analyzer_data.num_bins == 0 || nyquist_hz <= 0.0 {
        return 0.0;
    }

    let to_bin = |freq: f32| ((freq / nyquist_hz) * analyzer_data.num_bins as f32) as usize;
    let first = to_bin(low_hz).max(1);
    let last = (to_bin(high_hz) + 1).min(analyzer_data.num_bins);
    analyzer_data.gain_difference_db[first.min(last)..last]
        .iter()
        .fold(0.0f32, |peak, gain_change_db| peak.max(gain_change_db.abs()))
}

/// The spectrum, with the brake's work in the gap between the input and the output: amber where
/// the output is quieter than the input, a lighter amber where the brake lifted it. The rest of
/// the spectrum, the part both share, is drawn in the display's own spectrum colour on top.
fn draw_spectrum(
    cx: &mut DrawContext,
    canvas: &mut Canvas,
    analyzer_data: &AnalyzerData,
    nyquist_hz: f32,
) {
    let bounds = cx.bounds();
    if analyzer_data.num_bins < 2 || nyquist_hz <= 0.0 {
        return;
    }

    let spectrum_color: vg::Color = cx.font_color().into();
    let height_of = |db: f32| db_to_unclamped_t(db).clamp(0.0, 1.0);

    // One point per bin while the bins are spread out, then one per pixel column once they bunch
    // up, keeping the loudest bin in each column
    let mut points: Vec<(f32, f32, f32)> = Vec::with_capacity(bounds.w.ceil() as usize + 64);
    for bin_idx in 1..analyzer_data.num_bins {
        let freq = (bin_idx as f32 / analyzer_data.num_bins as f32) * nyquist_hz;
        let t = (freq.ln() - LN_FREQ_RANGE_START_HZ) / LN_FREQ_RANGE;
        if t <= 0.0 {
            continue;
        }
        if t >= 1.0 {
            break;
        }

        let input_db = nih_plug::util::gain_to_db(analyzer_data.envelope_followers[bin_idx]);
        let output_db = input_db + analyzer_data.gain_difference_db[bin_idx];
        let x = bounds.x + bounds.w * t;
        let (input_t, output_t) = (height_of(input_db), height_of(output_db));

        match points.last_mut() {
            Some(last) if x - last.0 < 1.0 => {
                last.1 = last.1.max(input_t);
                last.2 = last.2.max(output_t);
            }
            _ => points.push((x, input_t, output_t)),
        }
    }
    if points.is_empty() {
        return;
    }

    let mesh = |height: &dyn Fn(&(f32, f32, f32)) -> f32| {
        let bottom = bounds.y + bounds.h;
        let mut path = vg::Path::new();
        path.move_to(points[0].0, bottom);
        for point in &points {
            path.line_to(point.0, bounds.y + bounds.h * (1.0 - height(point)));
        }
        path.line_to(points[points.len() - 1].0, bottom);
        path.close();
        path
    };

    // Anti aliasing only causes seams between the three layers
    canvas.fill_path(
        &mesh(&|point| point.1),
        &vg::Paint::color(REDUCTION_COLOR).with_anti_alias(false),
    );
    canvas.fill_path(
        &mesh(&|point| point.2),
        &vg::Paint::color(LIFT_COLOR).with_anti_alias(false),
    );
    canvas.fill_path(
        &mesh(&|point| point.1.min(point.2)),
        &vg::Paint::color(spectrum_color).with_anti_alias(false),
    );
}
