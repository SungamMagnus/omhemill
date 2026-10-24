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
use std::cell::{Cell, RefCell};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::{BandSelection, Data, SoloEvent, BAND_COLORS};
use crate::analyzer::AnalyzerData;
use crate::curve::{octaves_from_q, BandShape, Curve, NUM_BANDS, RANGE_UNLIMITED_DB};
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

/// How fast the peak-hold line falls, in decibels per second.
const PEAK_FALL_DB_PER_SECOND: f32 = 12.0;
/// The gain change trace hangs from the top edge: this much gain change reaches
/// [`GAIN_TRACE_HEIGHT`] of the display's height.
const GAIN_TRACE_RANGE_DB: f32 = 24.0;
const GAIN_TRACE_HEIGHT: f32 = 0.16;
/// Below this a bin counts as silent.
const FLOOR_DB: f32 = -100.0;

/// How many pixels a decibel of gain change takes on a band's meter, and the meter's longest bar.
const METER_PX_PER_DB: f32 = 4.0;
const METER_MAX_PX: f32 = 80.0;

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
    /// The peak-hold line, in decibels, one value per column of the display.
    peaks_db: RefCell<Vec<f32>>,
    /// When the peak-hold line was last updated, so it falls at the same rate at any frame rate.
    last_peak_update: Cell<Option<Instant>>,
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
            peaks_db: RefCell::new(Vec::new()),
            last_peak_update: Cell::new(None),
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
            self.draw_spectrum(cx, canvas, analyzer_data, nyquist);
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

        // A fainter grid between the labelled lines: every step of each decade, and every 6 dB
        let mut minor = vg::Path::new();
        for decade in [10.0f32, 100.0, 1_000.0, 10_000.0] {
            for step in 2..10 {
                let t = Self::freq_to_t(decade * step as f32);
                if (0.0..=1.0).contains(&t) {
                    let x = (bounds.x + bounds.w * t).round() + 0.5;
                    minor.move_to(x, bounds.y);
                    minor.line_to(x, bounds.y + bounds.h);
                }
            }
        }
        for db in [-6.0f32, -18.0, -30.0, -42.0, -54.0, -66.0, -78.0] {
            let y = (bounds.y + bounds.h * (1.0 - db_to_unclamped_t(db))).round() + 0.5;
            minor.move_to(bounds.x, y);
            minor.line_to(bounds.x + bounds.w, y);
        }
        canvas.stroke_path(
            &minor,
            &vg::Paint::color(vg::Color::rgbaf(ink.r, ink.g, ink.b, 0.035)).with_line_width(scale),
        );

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
                    let meter_top = y + radius + 3.0 * scale;
                    let braking_db = peak_gain_change_db(analyzer_data, nyquist_hz, low_hz, high_hz);
                    if braking_db >= 0.2 {
                        let mut meter = vg::Path::new();
                        meter.rect(
                            x - 2.0 * scale,
                            meter_top,
                            4.0 * scale,
                            (braking_db * METER_PX_PER_DB).min(METER_MAX_PX) * scale,
                        );
                        canvas.fill_path(&meter, &vg::Paint::color(REDUCTION_COLOR));
                    }

                    // The band's range as a tick on the same scale, so you can see the meter stop
                    // at it
                    let range_db = self.params.threshold.bands[idx].range_db.value();
                    let range_px = range_db * METER_PX_PER_DB;
                    if range_db < RANGE_UNLIMITED_DB && range_px <= METER_MAX_PX {
                        let mut tick = vg::Path::new();
                        tick.rect(
                            x - 6.0 * scale,
                            meter_top + range_px * scale,
                            12.0 * scale,
                            1.5 * scale,
                        );
                        canvas.fill_path(&tick, &vg::Paint::color(color));
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

/// One column of the display: the input level, the gain change, and so the output level, in
/// decibels.
#[derive(Debug, Clone, Copy)]
struct Column {
    input_db: f32,
    gain_change_db: f32,
}

impl Column {
    fn output_db(&self) -> f32 {
        self.input_db + self.gain_change_db
    }
}

/// Samples the spectrum once per column. Where a column is narrower than a bin (the low end, on a
/// log scale) the neighbouring bins are interpolated so the curve stays smooth; where a column
/// holds several bins the loudest is kept, and the gain change with the largest magnitude.
fn sample_columns(analyzer_data: &AnalyzerData, nyquist_hz: f32, num_columns: usize) -> Vec<Column> {
    let num_bins = analyzer_data.num_bins;
    let bin_at = |t: f32| Analyzer::t_to_freq(t) / nyquist_hz * num_bins as f32;
    let input_db_at = |bin_idx: usize| {
        nih_plug::util::gain_to_db(analyzer_data.envelope_followers[bin_idx]).max(FLOOR_DB)
    };

    (0..num_columns)
        .map(|column_idx| {
            let first_bin = bin_at(column_idx as f32 / num_columns as f32);
            let last_bin = bin_at((column_idx + 1) as f32 / num_columns as f32);

            if last_bin - first_bin < 1.0 {
                let bin = bin_at((column_idx as f32 + 0.5) / num_columns as f32);
                let low = (bin.floor() as usize).clamp(1, num_bins - 2);
                let frac = (bin - low as f32).clamp(0.0, 1.0);
                let lerp = |a: f32, b: f32| a + (b - a) * frac;

                Column {
                    input_db: lerp(input_db_at(low), input_db_at(low + 1)),
                    gain_change_db: lerp(
                        analyzer_data.gain_difference_db[low],
                        analyzer_data.gain_difference_db[low + 1],
                    ),
                }
            } else {
                let first = (first_bin.ceil() as usize).clamp(1, num_bins - 1);
                let last = (last_bin.floor() as usize).clamp(first, num_bins - 1);
                let mut column = Column {
                    input_db: FLOOR_DB,
                    gain_change_db: 0.0,
                };
                for bin_idx in first..=last {
                    column.input_db = column.input_db.max(input_db_at(bin_idx));
                    let gain_change_db = analyzer_data.gain_difference_db[bin_idx];
                    if gain_change_db.abs() > column.gain_change_db.abs() {
                        column.gain_change_db = gain_change_db;
                    }
                }
                column
            }
        })
        .collect()
}

/// A closed shape from the bottom of `bounds` up to a height per column and back down.
fn column_mesh(bounds: BoundingBox, heights: impl Iterator<Item = f32>, num_columns: usize) -> vg::Path {
    let column_w = bounds.w / num_columns as f32;
    let bottom = bounds.y + bounds.h;
    let mut path = vg::Path::new();
    path.move_to(bounds.x, bottom);
    for (column_idx, height) in heights.enumerate() {
        let x = bounds.x + (column_idx as f32 + 0.5) * column_w;
        path.line_to(x, bounds.y + bounds.h * (1.0 - height));
    }
    path.line_to(bounds.x + bounds.w, bottom);
    path.close();
    path
}

/// An open line through a height per column.
fn column_line(bounds: BoundingBox, heights: impl Iterator<Item = f32>, num_columns: usize) -> vg::Path {
    let column_w = bounds.w / num_columns as f32;
    let mut path = vg::Path::new();
    for (column_idx, height) in heights.enumerate() {
        let x = bounds.x + (column_idx as f32 + 0.5) * column_w;
        let y = bounds.y + bounds.h * (1.0 - height);
        if column_idx == 0 {
            path.move_to(x, y);
        } else {
            path.line_to(x, y);
        }
    }
    path
}

impl Analyzer {
    /// The spectrum, with the compression's work in the gap between the input and the output:
    /// amber where the output is quieter than the input, a lighter amber where it was lifted.
    /// The part both share is shaded in the display's spectrum colour, with the output drawn as a
    /// crisp line, the input as a faint one, and a slowly falling peak-hold line above. Along the
    /// top edge hangs the gain change at every frequency, so the compression reads even where the
    /// signal is quiet.
    fn draw_spectrum(
        &self,
        cx: &mut DrawContext,
        canvas: &mut Canvas,
        analyzer_data: &AnalyzerData,
        nyquist_hz: f32,
    ) {
        let bounds = cx.bounds();
        if analyzer_data.num_bins < 3 || nyquist_hz <= 0.0 || bounds.w < 2.0 {
            return;
        }

        let scale = cx.scale_factor();
        let num_columns = (bounds.w / scale).ceil() as usize;
        let columns = sample_columns(analyzer_data, nyquist_hz, num_columns);
        let height_of = |db: f32| db_to_unclamped_t(db).clamp(0.0, 1.0);

        // The peak-hold line falls at a fixed rate whatever the frame rate
        let now = Instant::now();
        let elapsed = self
            .last_peak_update
            .replace(Some(now))
            .map(|last| now.duration_since(last).as_secs_f32().min(0.25))
            .unwrap_or(0.0);
        let mut peaks_db = self.peaks_db.borrow_mut();
        if peaks_db.len() != num_columns {
            *peaks_db = vec![FLOOR_DB; num_columns];
        }
        for (peak_db, column) in peaks_db.iter_mut().zip(&columns) {
            *peak_db = (*peak_db - PEAK_FALL_DB_PER_SECOND * elapsed).max(column.input_db);
        }

        let spectrum: vg::Color = cx.font_color().into();
        let ink: vg::Color = cx.border_color().into();
        let ink = |alpha: f32| vg::Color::rgbaf(ink.r, ink.g, ink.b, alpha);

        // Anti aliasing only causes seams between the stacked layers
        let reduction = vg::Paint::linear_gradient(
            bounds.x,
            bounds.y,
            bounds.x,
            bounds.y + bounds.h,
            vg::Color::rgbaf(REDUCTION_COLOR.r, REDUCTION_COLOR.g, REDUCTION_COLOR.b, 0.95),
            vg::Color::rgbaf(REDUCTION_COLOR.r, REDUCTION_COLOR.g, REDUCTION_COLOR.b, 0.55),
        )
        .with_anti_alias(false);
        canvas.fill_path(
            &column_mesh(bounds, columns.iter().map(|c| height_of(c.input_db)), num_columns),
            &reduction,
        );
        canvas.fill_path(
            &column_mesh(bounds, columns.iter().map(|c| height_of(c.output_db())), num_columns),
            &vg::Paint::color(LIFT_COLOR).with_anti_alias(false),
        );

        // The shared part, lighter towards the top so loud regions stand out. Both stops are
        // opaque so the amber only shows in the gap.
        let lighter = vg::Color::rgbf(
            (spectrum.r * 1.35).min(1.0),
            (spectrum.g * 1.35).min(1.0),
            (spectrum.b * 1.35).min(1.0),
        );
        let darker = vg::Color::rgbf(
            (spectrum.r + STAGE_COLOR.r) / 2.0,
            (spectrum.g + STAGE_COLOR.g) / 2.0,
            (spectrum.b + STAGE_COLOR.b) / 2.0,
        );
        canvas.fill_path(
            &column_mesh(
                bounds,
                columns
                    .iter()
                    .map(|c| height_of(c.input_db.min(c.output_db()))),
                num_columns,
            ),
            &vg::Paint::linear_gradient(
                bounds.x,
                bounds.y + bounds.h * 0.2,
                bounds.x,
                bounds.y + bounds.h,
                lighter,
                darker,
            )
            .with_anti_alias(false),
        );

        canvas.scissor(bounds.x, bounds.y, bounds.w, bounds.h);
        canvas.stroke_path(
            &column_line(bounds, peaks_db.iter().map(|db| height_of(*db)), num_columns),
            &vg::Paint::color(ink(0.22)).with_line_width(scale),
        );
        canvas.stroke_path(
            &column_line(bounds, columns.iter().map(|c| height_of(c.input_db)), num_columns),
            &vg::Paint::color(ink(0.30)).with_line_width(scale),
        );
        canvas.stroke_path(
            &column_line(bounds, columns.iter().map(|c| height_of(c.output_db())), num_columns),
            &vg::Paint::color(ink(0.62)).with_line_width(1.2 * scale),
        );

        // The gain change trace, hanging from the top edge
        let trace_y = |column: &Column| {
            let t = (column.gain_change_db.abs() / GAIN_TRACE_RANGE_DB).min(1.0);
            bounds.y + bounds.h * GAIN_TRACE_HEIGHT * t
        };
        let column_w = bounds.w / num_columns as f32;
        let mut trace_fill = vg::Path::new();
        let mut trace_line = vg::Path::new();
        trace_fill.move_to(bounds.x, bounds.y);
        for (column_idx, column) in columns.iter().enumerate() {
            let x = bounds.x + (column_idx as f32 + 0.5) * column_w;
            let y = trace_y(column);
            trace_fill.line_to(x, y);
            if column_idx == 0 {
                trace_line.move_to(x, y);
            } else {
                trace_line.line_to(x, y);
            }
        }
        trace_fill.line_to(bounds.x + bounds.w, bounds.y);
        trace_fill.close();
        canvas.fill_path(
            &trace_fill,
            &vg::Paint::color(vg::Color::rgbaf(
                REDUCTION_COLOR.r,
                REDUCTION_COLOR.g,
                REDUCTION_COLOR.b,
                0.16,
            )),
        );
        canvas.stroke_path(
            &trace_line,
            &vg::Paint::color(vg::Color::rgbaf(
                REDUCTION_COLOR.r,
                REDUCTION_COLOR.g,
                REDUCTION_COLOR.b,
                0.9,
            ))
            .with_line_width(1.2 * scale),
        );
        canvas.reset_scissor();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2048-point spectrum at 48 kHz, -40 dB everywhere, with -6 dB of gain change in the bins
    /// given.
    fn analyzer_data(loud_bin: usize, compressed_bins: std::ops::Range<usize>) -> AnalyzerData {
        let mut data = AnalyzerData::default();
        data.num_bins = 1025;
        data.envelope_followers[..1025].fill(nih_plug::util::db_to_gain(-40.0));
        data.envelope_followers[loud_bin] = nih_plug::util::db_to_gain(-10.0);
        data.gain_difference_db[compressed_bins].fill(-6.0);
        data
    }

    #[test]
    fn treble_columns_keep_the_loudest_bin_and_largest_gain_change() {
        // Bin 900 is about 21 kHz, where every column holds several bins
        let data = analyzer_data(900, 895..905);
        let columns = sample_columns(&data, 24_000.0, 800);

        let loudest = columns.iter().map(|c| c.input_db).fold(f32::MIN, f32::max);
        assert!((loudest - -10.0).abs() < 0.01);
        assert!(columns.iter().any(|c| (c.gain_change_db - -6.0).abs() < 1e-4));
    }

    #[test]
    fn bass_columns_interpolate_between_bins() {
        // Around 30-100 Hz a 2048-point bin spans many columns, so the curve is interpolated
        // rather than drawn in steps
        let data = analyzer_data(3, 0..0);
        let columns = sample_columns(&data, 24_000.0, 800);

        let low_end: Vec<f32> = columns[..120].iter().map(|c| c.input_db).collect();
        let distinct = low_end.windows(2).filter(|pair| (pair[0] - pair[1]).abs() > 1e-3).count();
        assert!(distinct > 20, "the low end should change smoothly, not in a few steps");
        assert!(low_end.iter().all(|db| (-40.01..=-9.99).contains(db)));
    }
}
