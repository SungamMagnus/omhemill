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
use nih_plug::nih_debug_assert;
use nih_plug_vizia::vizia::prelude::*;
use nih_plug_vizia::vizia::vg;
use nih_plug_vizia::widgets::param_base::ParamWidgetBase;
use nih_plug_vizia::widgets::util::ModifiersExt;
use std::cell::Cell;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use crate::analyzer::AnalyzerData;
use crate::curve::{Curve, NUM_BANDS};
use crate::OmhemillParams;

/// The sensitivity line is the thing you are actually shaping, so it is drawn in full ink.
const SENSITIVITY_LINE_COLOR: vg::Color = vg::Color::rgbaf(0.925, 0.910, 0.875, 1.0);
/// The sensitivity line has its own scale, centred on the display like an EQ's gain axis: this
/// many decibels of sensitivity reach [`SENSITIVITY_HALF_HEIGHT`] of the way to the edge.
const SENSITIVITY_RANGE_DB: f32 = 18.0;
const SENSITIVITY_HALF_HEIGHT: f32 = 0.45;
/// Band handles are coral, the colour of detection.
const NODE_COLOR: vg::Color = vg::Color::rgbaf(0.929, 0.506, 0.349, 1.0);
const NODE_DISABLED_COLOR: vg::Color = vg::Color::rgbaf(0.925, 0.910, 0.875, 0.38);
/// The stage the spectrum sits on, used to punch the number out of a filled handle.
const STAGE_COLOR: vg::Color = vg::Color::rgbaf(0.125, 0.122, 0.118, 1.0);
/// Half the side of a (square) band handle.
const NODE_RADIUS: f32 = 10.0;

// We'll show the bins from 30 Hz (to your chest) to 22 kHz, scaled logarithmically
#[allow(unused)]
const FREQ_RANGE_START_HZ: f32 = 30.0;
#[allow(unused)]
const FREQ_RANGE_END_HZ: f32 = 22_000.0;
const LN_FREQ_RANGE_START_HZ: f32 = 3.4011974; // 30.0f32.ln();
const LN_FREQ_RANGE_END_HZ: f32 = 9.998797; // 22_000.0f32.ln();
const LN_FREQ_RANGE: f32 = LN_FREQ_RANGE_END_HZ - LN_FREQ_RANGE_START_HZ;

/// The gain change overlay. Amber is reserved for live state: this is the brake working right now.
const GR_BAR_OVERLAY_COLOR: vg::Color = vg::Color::rgbaf(0.851, 0.639, 0.161, 0.72);

/// A very analyzer showing the envelope followers as a magnitude spectrum with an overlay for the
/// gain reduction.
/// The parameter handles for a single draggable band, so the analyzer can move it around while
/// still producing proper automation gestures.
struct BandHandles {
    enabled: ParamWidgetBase,
    frequency: ParamWidgetBase,
    sensitivity_db: ParamWidgetBase,
    width: ParamWidgetBase,
}

pub struct Analyzer {
    analyzer_data: Arc<Mutex<triple_buffer::Output<AnalyzerData>>>,
    sample_rate: Arc<AtomicF32>,
    /// Read during drawing to place the band handles.
    params: Arc<OmhemillParams>,
    /// Used to actually move a band when it gets dragged.
    bands: Vec<BandHandles>,
    /// The band currently being dragged, if any.
    dragged_band: Option<usize>,
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
    ) -> Handle<Self>
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
                frequency: ParamWidgetBase::new(cx, params.clone(), move |p| {
                    &p.threshold.bands[idx].frequency
                }),
                sensitivity_db: ParamWidgetBase::new(cx, params.clone(), move |p| {
                    &p.threshold.bands[idx].sensitivity_db
                }),
                width: ParamWidgetBase::new(cx, params.clone(), move |p| {
                    &p.threshold.bands[idx].width
                }),
            })
            .collect();

        Self {
            analyzer_data: analyzer_data.get(cx),
            sample_rate: sample_rate.get(cx),
            params: params.get(cx),
            bands,
            dragged_band: None,
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
    fn freq_to_t(freq: f32) -> f32 {
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
                // A cut has no sensitivity of its own, so it sits on the zero line
                let sensitivity_db = if is_cut {
                    0.0
                } else {
                    curve.sensitivity_ln(freq.ln())
                };
                let y = sensitivity_to_y(bounds, sensitivity_db);

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
    /// translated back into a sensitivity for that band.
    fn sensitivity_without_band(&self, freq: f32, skip_idx: usize) -> f32 {
        let mut curve_params = self.params.threshold.curve_params();
        curve_params.bands[skip_idx].enabled = false;

        Curve::new(&curve_params).sensitivity_ln(freq.ln())
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
                if let Some(idx) = self.band_at(bounds, mouse_x, mouse_y, cx.scale_factor()) {
                    let band = &self.bands[idx];

                    // Dragging a disabled band switches it on, which is what you meant by
                    // grabbing it
                    if self.params.threshold.bands[idx].enabled.value() {
                        self.dragged_band = Some(idx);
                        cx.capture();
                        cx.set_active(true);

                        band.frequency.begin_set_parameter(cx);
                        band.sensitivity_db.begin_set_parameter(cx);
                    } else {
                        band.enabled.begin_set_parameter(cx);
                        band.enabled.set_normalized_value(cx, 1.0);
                        band.enabled.end_set_parameter(cx);
                    }

                    meta.consume();
                }
            }
            WindowEvent::MouseDoubleClick(MouseButton::Left) => {
                // A double click switches the band under the pointer back off again
                let (mouse_x, mouse_y) = (cx.mouse().cursorx, cx.mouse().cursory);
                if let Some(idx) = self.band_at(bounds, mouse_x, mouse_y, cx.scale_factor()) {
                    if self.params.threshold.bands[idx].enabled.value() {
                        let band = &self.bands[idx];
                        band.enabled.begin_set_parameter(cx);
                        band.enabled.set_normalized_value(cx, 0.0);
                        band.enabled.end_set_parameter(cx);

                        meta.consume();
                    }
                }
            }
            WindowEvent::MouseUp(MouseButton::Left) => {
                if let Some(idx) = self.dragged_band.take() {
                    let band = &self.bands[idx];
                    band.frequency.end_set_parameter(cx);
                    band.sensitivity_db.end_set_parameter(cx);

                    cx.release();
                    cx.set_active(false);

                    meta.consume();
                }
            }
            WindowEvent::MouseMove(x, y) => {
                if let Some(idx) = self.dragged_band {
                    if bounds.w <= 0.0 || bounds.h <= 0.0 {
                        return;
                    }

                    // Horizontally the pointer picks the frequency directly
                    let t = ((*x - bounds.x) / bounds.w).clamp(0.0, 1.0);
                    let freq = Self::t_to_freq(t);
                    let band = &self.bands[idx];
                    band.frequency
                        .set_normalized_value(cx, band.frequency.preview_normalized(freq));

                    // Vertically it says where the sensitivity line should end up, minus what the
                    // other bands already add there. A cut has no sensitivity, so it only moves
                    // sideways.
                    if !self.params.threshold.bands[idx].shape.value().is_cut() {
                        let target_db = y_to_sensitivity(bounds, *y);
                        let sensitivity_db = target_db - self.sensitivity_without_band(freq, idx);
                        band.sensitivity_db.set_normalized_value(
                            cx,
                            band.sensitivity_db.preview_normalized(sensitivity_db),
                        );
                    }
                }
            }
            WindowEvent::MouseScroll(_scroll_x, scroll_y) => {
                // Scrolling over a band changes its width
                let (mouse_x, mouse_y) = (cx.mouse().cursorx, cx.mouse().cursory);
                let hovered = self
                    .dragged_band
                    .or_else(|| self.band_at(bounds, mouse_x, mouse_y, cx.scale_factor()));
                if let Some(idx) = hovered {
                    self.scrolled_lines += scroll_y;
                    if self.scrolled_lines.abs() >= 1.0 {
                        let width = &self.bands[idx].width;
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

        // The analyzer data is pulled directly from the spectral `CompressorBank`
        let mut analyzer_data = self.analyzer_data.lock().unwrap();
        let analyzer_data = analyzer_data.read();
        let nyquist = self.sample_rate.load(Ordering::Relaxed) / 2.0;

        self.draw_grid(cx, canvas);
        draw_spectrum(cx, canvas, analyzer_data, nyquist);
        draw_gain_reduction(cx, canvas, analyzer_data, nyquist);
        self.draw_sensitivity_line(cx, canvas);
        self.draw_bands(cx, canvas);
        self.draw_legend(cx, canvas);

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

    /// A light reference grid with frequency and decibel labels, drawn behind everything else.
    fn draw_grid(&self, cx: &mut DrawContext, canvas: &mut Canvas) {
        let bounds = cx.bounds();
        let scale = cx.scale_factor();

        let ink: vg::Color = cx.border_color().into();
        let grid_paint = vg::Paint::color(vg::Color::rgbaf(ink.r, ink.g, ink.b, 0.07))
            .with_line_width(scale);

        let label_color = vg::Color::rgbaf(ink.r, ink.g, ink.b, 0.45);
        let font = self.font(canvas);

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

            if let Some(font) = font {
                let mut paint = vg::Paint::color(label_color);
                paint.set_font(&[font]);
                paint.set_font_size(10.0 * scale);
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

            if let Some(font) = font {
                let mut paint = vg::Paint::color(label_color);
                paint.set_font(&[font]);
                paint.set_font_size(10.0 * scale);
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

        // The sensitivity scale on the left, and its zero line dashed across the display
        let mut zero_line = vg::Path::new();
        let zero_y = sensitivity_to_y(bounds, 0.0).round() + 0.5;
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

        if let Some(font) = font {
            for sensitivity_db in [12.0f32, 0.0, -12.0] {
                let mut paint = vg::Paint::color(label_color);
                paint.set_font(&[font]);
                paint.set_font_size(10.0 * scale);
                paint.set_text_align(vg::Align::Left);
                paint.set_text_baseline(vg::Baseline::Middle);
                let label = if sensitivity_db > 0.0 {
                    format!("+{sensitivity_db:.0}")
                } else {
                    format!("{sensitivity_db:.0}")
                };
                let _ = canvas.fill_text(
                    bounds.x + (6.0 * scale),
                    sensitivity_to_y(bounds, sensitivity_db),
                    label,
                    &paint,
                );
            }
        }
    }

    /// The sensitivity line: flat at zero until a band bends it, like an EQ's response curve.
    /// Where the cuts (or a band set to Off) fence the brake off, the stage is dimmed and the line
    /// drops out. This reads the parameters directly so it follows a drag without waiting for the
    /// audio thread.
    fn draw_sensitivity_line(&self, cx: &mut DrawContext, canvas: &mut Canvas) {
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

            let y = sensitivity_to_y(bounds, shape.sensitivity_db);
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
            &vg::Paint::color(SENSITIVITY_LINE_COLOR).with_line_width(2.0 * scale),
        );
        canvas.reset_scissor();
    }

    /// A key for the three things drawn on the graph, in the top right corner.
    fn draw_legend(&self, cx: &mut DrawContext, canvas: &mut Canvas) {
        let Some(font) = self.font(canvas) else {
            return;
        };

        let bounds = cx.bounds();
        let scale = cx.scale_factor();
        let spectrum_color: vg::Color = cx.font_color().into();
        let ink: vg::Color = cx.border_color().into();
        let ink = vg::Color::rgbaf(ink.r, ink.g, ink.b, 1.0);

        let entries = [
            ("REDUCTION", vg::Color::rgbaf(0.851, 0.639, 0.161, 1.0)),
            ("SENSITIVITY", SENSITIVITY_LINE_COLOR),
            ("SPECTRUM", spectrum_color),
        ];

        let mut paint = vg::Paint::color(ink);
        paint.set_font(&[font]);
        paint.set_font_size(9.0 * scale);
        paint.set_text_align(vg::Align::Right);
        paint.set_text_baseline(vg::Baseline::Middle);

        let y = bounds.y + (14.0 * scale);
        let mut right = bounds.x + bounds.w - (12.0 * scale);

        for (label, color) in entries {
            let mut text_paint = paint.clone();
            text_paint.set_color(vg::Color::rgbaf(ink.r, ink.g, ink.b, 0.55));
            let text_width = canvas
                .fill_text(right, y, label, &text_paint)
                .map(|metrics| metrics.width())
                .unwrap_or(50.0 * scale);

            // A short dash in the thing's own colour, just left of its label
            let dash_right = right - text_width - (6.0 * scale);
            let mut dash = vg::Path::new();
            dash.move_to(dash_right - (12.0 * scale), y);
            dash.line_to(dash_right, y);
            canvas.stroke_path(
                &dash,
                &vg::Paint::color(color).with_line_width(2.0 * scale),
            );

            right = dash_right - (12.0 * scale) - (14.0 * scale);
        }
    }

    /// The numbered band handles sitting on the threshold curve, each with a drop line down to
    /// the bottom of the graph so you can see which frequency it is parked on. Cuts are drawn
    /// hollow: they fence the brake off rather than shape it.
    fn draw_bands(&self, cx: &mut DrawContext, canvas: &mut Canvas) {
        let bounds = cx.bounds();
        let scale = cx.scale_factor();
        let radius = NODE_RADIUS * scale;
        let font = self.font(canvas);

        canvas.scissor(bounds.x, bounds.y, bounds.w, bounds.h);

        for (idx, (x, y, enabled, is_cut)) in self.band_positions(bounds).into_iter().enumerate() {
            let color = if enabled {
                NODE_COLOR
            } else {
                NODE_DISABLED_COLOR
            };

            if enabled {
                let mut drop = vg::Path::new();
                drop.move_to(x, y);
                drop.line_to(x, bounds.y + bounds.h);
                canvas.stroke_path(
                    &drop,
                    &vg::Paint::color(vg::Color::rgbaf(color.r, color.g, color.b, 0.35))
                        .with_line_width(scale),
                );
            }

            // Square, like every other corner in the system
            let mut handle = vg::Path::new();
            handle.rect(x - radius, y - radius, radius * 2.0, radius * 2.0);
            let filled = enabled && !is_cut;
            if filled {
                canvas.fill_path(&handle, &vg::Paint::color(color));
            } else {
                canvas.fill_path(&handle, &vg::Paint::color(STAGE_COLOR));
            }
            canvas.stroke_path(
                &handle,
                &vg::Paint::color(color).with_line_width(1.4 * scale),
            );

            if let Some(font) = font {
                let text_color = if filled { STAGE_COLOR } else { color };

                let mut paint = vg::Paint::color(text_color);
                paint.set_font(&[font]);
                paint.set_font_size(11.0 * scale);
                paint.set_text_align(vg::Align::Center);
                paint.set_text_baseline(vg::Baseline::Middle);
                let _ = canvas.fill_text(x, y, format!("{}", idx + 1), &paint);
            }
        }

        canvas.reset_scissor();
    }
}

/// Draw the spectrum analyzer part of the analyzer. These are drawn as vertical bars until the
/// spacing between the bars becomes less the line width, at which point it's drawn as a solid mesh
/// instead.
fn draw_spectrum(
    cx: &mut DrawContext,
    canvas: &mut Canvas,
    analyzer_data: &AnalyzerData,
    nyquist_hz: f32,
) {
    let bounds = cx.bounds();

    let line_width = cx.scale_factor() * 1.5;
    let text_color: vg::Color = cx.font_color().into();
    // This is used to draw the individual bars
    let bars_paint = vg::Paint::color(text_color).with_line_width(line_width);
    // And this color is used to draw the mesh part of the spectrum. We'll create a gradient paint
    // that fades from this to `text_color` when we know the mesh's x-coordinates.
    let mut lighter_text_color = text_color;
    lighter_text_color.r = (lighter_text_color.r + 0.25) / 1.25;
    lighter_text_color.g = (lighter_text_color.g + 0.25) / 1.25;
    lighter_text_color.b = (lighter_text_color.b + 0.25) / 1.25;

    // The frequency belonging to a bin in Hz
    let bin_frequency = |bin_idx: f32| (bin_idx / analyzer_data.num_bins as f32) * nyquist_hz;
    // A `[0, 1]` value indicating at which relative x-coordinate a bin should be drawn at
    let bin_t =
        |bin_idx: f32| (bin_frequency(bin_idx).ln() - LN_FREQ_RANGE_START_HZ) / LN_FREQ_RANGE;
    // Converts a linear magnitude value in to a `[0, 1]` value where 0 is -80 dB or lower, and 1 is
    // +20 dB or higher.
    let magnitude_height = |magnitude: f32| {
        nih_debug_assert!(magnitude >= 0.0);
        let magnitude_db = nih_plug::util::gain_to_db(magnitude);
        db_to_unclamped_t(magnitude_db).clamp(0.0, 1.0)
    };

    // The first part of this drawing routing is simple. Individual bins are drawn as bars until the
    // distance between the bars approaches `mesh_start_delta_threshold`. After that the rest is
    // drawn as a solid mesh.
    let mesh_start_delta_threshold = line_width + 0.5;
    let mut mesh_bin_start_idx = analyzer_data.num_bins;
    let mut previous_physical_x_coord = bounds.x - 2.0;

    let mut bars_path = vg::Path::new();
    for (bin_idx, magnitude) in analyzer_data
        .envelope_followers
        .iter()
        .enumerate()
        .take(analyzer_data.num_bins)
    {
        let t = bin_t(bin_idx as f32);
        if t <= 0.0 || t >= 1.0 {
            continue;
        }

        let physical_x_coord = bounds.x + (bounds.w * t);
        if physical_x_coord - previous_physical_x_coord < mesh_start_delta_threshold {
            // NOTE: We'll draw this one bar earlier because we're not stroking the solid mesh part,
            //       and otherwise there would be a weird looking gap at the left side
            mesh_bin_start_idx = bin_idx.saturating_sub(1);
            previous_physical_x_coord = physical_x_coord;
            break;
        }

        // Scale this so that 1.0/0 dBFS magnitude is at 80% of the height, the bars begin
        // at -80 dBFS, and that the scaling is linear. This is the same scaling used in
        // Diopser's spectrum analyzer.
        let height = magnitude_height(*magnitude);

        bars_path.move_to(physical_x_coord, bounds.y + (bounds.h * (1.0 - height)));
        bars_path.line_to(physical_x_coord, bounds.y + bounds.h);

        previous_physical_x_coord = physical_x_coord;
    }
    canvas.stroke_path(&bars_path, &bars_paint);

    // The mesh path starts at the bottom left, follows the top envelope of the spectrum analyzer,
    // and ends in the bottom right
    let mut mesh_path = vg::Path::new();
    let mesh_start_x_coordiante = bounds.x + (bounds.w * bin_t(mesh_bin_start_idx as f32));
    let mesh_start_y_coordinate = bounds.y + bounds.h;

    mesh_path.move_to(mesh_start_x_coordiante, mesh_start_y_coordinate);
    for (bin_idx, magnitude) in analyzer_data
        .envelope_followers
        .iter()
        .enumerate()
        .take(analyzer_data.num_bins)
        .skip(mesh_bin_start_idx)
    {
        let t = bin_t(bin_idx as f32);
        if t <= 0.0 || t >= 1.0 {
            continue;
        }

        let physical_x_coord = bounds.x + (bounds.w * t);
        previous_physical_x_coord = physical_x_coord;
        let height = magnitude_height(*magnitude);
        if height > 0.0 {
            mesh_path.line_to(
                physical_x_coord,
                // This includes the line width, since this path is not stroked
                bounds.y + (bounds.h * (1.0 - height) - (line_width / 2.0)).max(0.0),
            );
        } else {
            mesh_path.line_to(physical_x_coord, mesh_start_y_coordinate);
        }
    }

    mesh_path.line_to(previous_physical_x_coord, mesh_start_y_coordinate);
    mesh_path.close();

    let mesh_paint = vg::Paint::linear_gradient_stops(
        mesh_start_x_coordiante,
        0.0,
        previous_physical_x_coord,
        0.0,
        [
            (0.0, lighter_text_color),
            (0.707, text_color),
            (1.0, text_color),
        ],
    )
    // NOTE:  This is very important, otherwise this looks all kinds of gnarly
    .with_anti_alias(false);
    canvas.fill_path(&mesh_path, &mesh_paint);
}

/// Where a sensitivity value sits vertically within `bounds`.
fn sensitivity_to_y(bounds: BoundingBox, sensitivity_db: f32) -> f32 {
    let t = (sensitivity_db / SENSITIVITY_RANGE_DB).clamp(-1.0, 1.0) * SENSITIVITY_HALF_HEIGHT;
    bounds.y + bounds.h * (0.5 - t)
}

/// The inverse of [`sensitivity_to_y()`].
fn y_to_sensitivity(bounds: BoundingBox, y: f32) -> f32 {
    let t = 0.5 - ((y - bounds.y) / bounds.h);
    (t / SENSITIVITY_HALF_HEIGHT) * SENSITIVITY_RANGE_DB
}

/// Overlays the gain reduction display over the spectrum analyzer.
fn draw_gain_reduction(
    cx: &mut DrawContext,
    canvas: &mut Canvas,
    analyzer_data: &AnalyzerData,
    nyquist_hz: f32,
) {
    let bounds = cx.bounds();

    // As with the above, anti aliasing only causes issues
    let paint = vg::Paint::color(GR_BAR_OVERLAY_COLOR).with_anti_alias(false);

    let bin_frequency = |bin_idx: f32| (bin_idx / analyzer_data.num_bins as f32) * nyquist_hz;

    let mut path = vg::Path::new();
    for (bin_idx, gain_difference_db) in analyzer_data
        .gain_difference_db
        .iter()
        .enumerate()
        .take(analyzer_data.num_bins)
    {
        // Avoid drawing tiny slivers for low gain reduction values
        if gain_difference_db.abs() < 0.2 {
            continue;
        }

        // The gain reduction bars are drawn with the width of the bin, centered on the bin's center
        // frequency. The first and the last bin are extended to the edges of the graph because
        // otherwise it looks weird.
        let t_start = if bin_idx == 0 {
            0.0
        } else {
            let gr_start_ln_frequency = bin_frequency(bin_idx as f32 - 0.5).ln();
            (gr_start_ln_frequency - LN_FREQ_RANGE_START_HZ) / LN_FREQ_RANGE
        };
        let t_end = if bin_idx == analyzer_data.num_bins - 1 {
            1.0
        } else {
            let gr_end_ln_frequency = bin_frequency(bin_idx as f32 + 0.5).ln();
            (gr_end_ln_frequency - LN_FREQ_RANGE_START_HZ) / LN_FREQ_RANGE
        };
        if t_end < 0.0 || t_start > 1.0 {
            continue;
        }

        let (t_start, t_end) = (t_start.max(0.0), t_end.min(1.0));

        // For the bar's height we'll draw 0 dB of gain reduction as a flat line (except we
        // don't actually draw 0 dBs of GR because it looks glitchy, but that's besides the
        // point). 40 dB of gain reduction causes the bar to be drawn from the center all
        // the way to the bottom of the spectrum analyzer. 40 dB of additional gain causes
        // the bar to be drawn from the center all the way to the top of the graph.
        // NOTE: Y-coordinates go from top to bottom, hence the minus
        let t_y = ((-gain_difference_db + 40.0) / 80.0).clamp(0.0, 1.0);

        path.move_to(bounds.x + (bounds.w * t_start), bounds.y + (bounds.h * 0.5));
        path.line_to(bounds.x + (bounds.w * t_end), bounds.y + (bounds.h * 0.5));
        path.line_to(bounds.x + (bounds.w * t_end), bounds.y + (bounds.h * t_y));
        path.line_to(bounds.x + (bounds.w * t_start), bounds.y + (bounds.h * t_y));
        path.close();
    }

    canvas.fill_path(&path, &paint);
}
