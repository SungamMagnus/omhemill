// Ómhemill: the graphite edition's two parameter controls, built against nih_plug_vizia's
// ParamWidgetBase like the stock widgets, so automation gestures and host modulation behave the
// same way.
//
// - `BarSlider` is the Sungam bar: a hairline box with a translucent fill in its section's colour
//   and the value written at the left in that colour. Bipolar parameters fill outwards from their
//   default instead of from the left edge.
// - `SegmentButton` is one cell of a segmented selector for a stepped parameter.
// - `ShapeIcon` draws a band shape, for the shape picker's segment buttons.
// - `ResolutionButton` sets the STFT window size and overlap together from a few named steps.
//
// The stock ParamSlider centres its value text with an inline style the stylesheet cannot
// override, which is why the bar is its own widget.

use nih_plug::prelude::Param;

use super::Data;
use crate::curve::BandShape;
use nih_plug_vizia::vizia::prelude::*;
use nih_plug_vizia::vizia::vg;
use nih_plug_vizia::widgets::param_base::ParamWidgetBase;
use nih_plug_vizia::widgets::util::{self, ModifiersExt};

/// Shift-dragging moves the value this many times slower than the pointer.
const GRANULAR_DRAG_MULTIPLIER: f32 = 0.1;

#[derive(Debug, Clone, Copy)]
struct GranularDragStatus {
    starting_x_coordinate: f32,
    starting_value: f32,
}

/// A horizontal bar bound to a NIH-plug parameter. Click or drag to set the value, shift-drag for
/// finer control, scroll to nudge, and double-click (or right-click, or command-click) to reset.
pub struct BarSlider {
    param_base: ParamWidgetBase,
    drag_active: bool,
    granular_drag_status: Option<GranularDragStatus>,
    scrolled_lines: f32,
}

impl BarSlider {
    pub fn new<L, Params, P, FMap>(
        cx: &mut Context,
        params: L,
        params_to_param: FMap,
    ) -> Handle<'_, Self>
    where
        L: Lens<Target = Params> + Clone,
        Params: 'static,
        P: Param + 'static,
        FMap: Fn(&Params) -> &P + Copy + 'static,
    {
        Self {
            param_base: ParamWidgetBase::new(cx, params.clone(), params_to_param),
            drag_active: false,
            granular_drag_status: None,
            scrolled_lines: 0.0,
        }
        .build(
            cx,
            ParamWidgetBase::build_view(params, params_to_param, |cx, param_data| {
                let display_value_lens = param_data.make_lens(|param| {
                    param.normalized_value_to_string(param.unmodulated_normalized_value(), true)
                });
                Label::new(cx, display_value_lens)
                    .class("bar-value")
                    .hoverable(false);
            }),
        )
    }

    fn reset_to_default(&self, cx: &mut EventContext) {
        self.param_base.begin_set_parameter(cx);
        self.param_base
            .set_normalized_value(cx, self.param_base.default_normalized_value());
        self.param_base.end_set_parameter(cx);
    }

    /// Continuous parameters whose default sits somewhere in the middle (tilt, output gain) read
    /// as a deviation from that default.
    fn is_bipolar(&self) -> bool {
        let default = self.param_base.default_normalized_value();
        self.param_base.step_count().is_none() && default > 0.05 && default < 0.95
    }
}

impl View for BarSlider {
    fn element(&self) -> Option<&'static str> {
        Some("bar-slider")
    }

    fn event(&mut self, cx: &mut EventContext, event: &mut Event) {
        event.map(|window_event, meta| match window_event {
            // As in the stock slider, a third click counts as a regular click so double click
            // followed by a drag works
            WindowEvent::MouseDown(MouseButton::Left)
            | WindowEvent::MouseTripleClick(MouseButton::Left) => {
                if cx.modifiers().command() {
                    self.reset_to_default(cx);
                } else {
                    self.drag_active = true;
                    cx.capture();
                    cx.focus();
                    cx.set_active(true);

                    self.param_base.begin_set_parameter(cx);
                    if cx.modifiers().shift() {
                        self.granular_drag_status = Some(GranularDragStatus {
                            starting_x_coordinate: cx.mouse().cursorx,
                            starting_value: self.param_base.unmodulated_normalized_value(),
                        });
                    } else {
                        self.granular_drag_status = None;
                        self.param_base.set_normalized_value(
                            cx,
                            util::remap_current_entity_x_coordinate(cx, cx.mouse().cursorx),
                        );
                    }
                }

                meta.consume();
            }
            WindowEvent::MouseDoubleClick(MouseButton::Left)
            | WindowEvent::MouseDown(MouseButton::Right)
            | WindowEvent::MouseDoubleClick(MouseButton::Right)
            | WindowEvent::MouseTripleClick(MouseButton::Right) => {
                self.reset_to_default(cx);
                meta.consume();
            }
            WindowEvent::MouseUp(MouseButton::Left) => {
                if self.drag_active {
                    self.drag_active = false;
                    self.granular_drag_status = None;
                    cx.release();
                    cx.set_active(false);

                    self.param_base.end_set_parameter(cx);

                    meta.consume();
                }
            }
            WindowEvent::MouseMove(x, _y) => {
                if self.drag_active {
                    if cx.modifiers().shift() {
                        let status = *self.granular_drag_status.get_or_insert_with(|| {
                            GranularDragStatus {
                                starting_x_coordinate: *x,
                                starting_value: self.param_base.unmodulated_normalized_value(),
                            }
                        });

                        let start_x = util::remap_current_entity_x_t(cx, status.starting_value);
                        let delta_x = (*x - status.starting_x_coordinate) * GRANULAR_DRAG_MULTIPLIER;
                        self.param_base.set_normalized_value(
                            cx,
                            util::remap_current_entity_x_coordinate(cx, start_x + delta_x),
                        );
                    } else {
                        self.granular_drag_status = None;
                        self.param_base.set_normalized_value(
                            cx,
                            util::remap_current_entity_x_coordinate(cx, *x),
                        );
                    }
                }
            }
            WindowEvent::MouseScroll(_scroll_x, scroll_y) => {
                self.scrolled_lines += scroll_y;

                if self.scrolled_lines.abs() >= 1.0 {
                    let use_finer_steps = cx.modifiers().shift();

                    if !self.drag_active {
                        self.param_base.begin_set_parameter(cx);
                    }

                    let mut current_value = self.param_base.unmodulated_normalized_value();
                    while self.scrolled_lines >= 1.0 {
                        current_value = self
                            .param_base
                            .next_normalized_step(current_value, use_finer_steps);
                        self.scrolled_lines -= 1.0;
                    }
                    while self.scrolled_lines <= -1.0 {
                        current_value = self
                            .param_base
                            .previous_normalized_step(current_value, use_finer_steps);
                        self.scrolled_lines += 1.0;
                    }

                    self.param_base.set_normalized_value(cx, current_value);

                    if !self.drag_active {
                        self.param_base.end_set_parameter(cx);
                    }
                }

                meta.consume();
            }
            _ => {}
        });
    }

    fn draw(&self, cx: &mut DrawContext, canvas: &mut Canvas) {
        let bounds = cx.bounds();
        if bounds.w == 0.0 || bounds.h == 0.0 {
            return;
        }

        let scale = cx.scale_factor();
        let background: vg::Color = cx.background_color().into();
        let border: vg::Color = cx.border_color().into();
        // The section colour arrives as the text colour, so the value label picks it up too
        let accent: vg::Color = cx.font_color().into();

        let mut body = vg::Path::new();
        body.rect(bounds.x, bounds.y, bounds.w, bounds.h);
        canvas.fill_path(&body, &vg::Paint::color(background));

        let value = self.param_base.unmodulated_normalized_value();
        let (start_t, end_t) = if self.is_bipolar() {
            let default = self.param_base.default_normalized_value();
            (default.min(value), default.max(value))
        } else {
            (0.0, value)
        };
        if end_t > start_t {
            let mut fill = vg::Path::new();
            fill.rect(
                bounds.x + bounds.w * start_t,
                bounds.y,
                bounds.w * (end_t - start_t),
                bounds.h,
            );
            canvas.fill_path(
                &fill,
                &vg::Paint::color(vg::Color::rgbaf(accent.r, accent.g, accent.b, 0.32)),
            );
        }

        if self.is_bipolar() {
            let x = (bounds.x + bounds.w * self.param_base.default_normalized_value()).round();
            let mut tick = vg::Path::new();
            tick.move_to(x + 0.5, bounds.y);
            tick.line_to(x + 0.5, bounds.y + bounds.h);
            canvas.stroke_path(&tick, &vg::Paint::color(border).with_line_width(scale));
        }

        let line_width = scale;
        let mut outline = vg::Path::new();
        outline.rect(
            bounds.x + line_width / 2.0,
            bounds.y + line_width / 2.0,
            bounds.w - line_width,
            bounds.h - line_width,
        );
        canvas.stroke_path(&outline, &vg::Paint::color(border).with_line_width(line_width));
    }
}

/// A [`BarSlider`] with its name in small caps above it. `accent` is a stylesheet class that
/// sets the section colour.
pub fn labeled_bar<L, Params, P, FMap>(
    cx: &mut Context,
    params: L,
    params_to_param: FMap,
    label: &str,
    accent: &str,
) where
    L: Lens<Target = Params> + Clone,
    Params: 'static,
    P: Param + 'static,
    FMap: Fn(&Params) -> &P + Copy + 'static,
{
    VStack::new(cx, |cx| {
        Label::new(cx, label).class("bar-name");
        BarSlider::new(cx, params, params_to_param).class(accent);
    })
    .class("bar-cell");
}

/// One cell of a segmented selector. Clicking it sets the parameter to `normalized_value`, and
/// it reads as checked while the parameter sits there.
pub struct SegmentButton {
    param_base: ParamWidgetBase,
    normalized_value: f32,
}

impl SegmentButton {
    pub fn new<'a, L, Params, P, FMap>(
        cx: &'a mut Context,
        params: L,
        params_to_param: FMap,
        normalized_value: f32,
        label: &str,
    ) -> Handle<'a, Self>
    where
        L: Lens<Target = Params> + Clone,
        Params: 'static,
        P: Param + 'static,
        FMap: Fn(&Params) -> &P + Copy + 'static,
    {
        let label = label.to_owned();
        Self::with_content(cx, params, params_to_param, normalized_value, move |cx| {
            Label::new(cx, &label).class("segment-label").hoverable(false);
        })
    }

    /// A segment showing something other than a text label, such as a [`ShapeIcon`].
    pub fn with_content<'a, L, Params, P, FMap>(
        cx: &'a mut Context,
        params: L,
        params_to_param: FMap,
        normalized_value: f32,
        content: impl FnOnce(&mut Context),
    ) -> Handle<'a, Self>
    where
        L: Lens<Target = Params> + Clone,
        Params: 'static,
        P: Param + 'static,
        FMap: Fn(&Params) -> &P + Copy + 'static,
    {
        let is_selected = ParamWidgetBase::make_lens(params.clone(), params_to_param, move |param| {
            (param.unmodulated_normalized_value() - normalized_value).abs() < 1e-3
        });

        Self {
            param_base: ParamWidgetBase::new(cx, params, params_to_param),
            normalized_value,
        }
        .build(cx, content)
        .checked(is_selected)
    }
}

impl View for SegmentButton {
    fn element(&self) -> Option<&'static str> {
        Some("segment-button")
    }

    fn event(&mut self, cx: &mut EventContext, event: &mut Event) {
        event.map(|window_event, meta| match window_event {
            WindowEvent::MouseDown(MouseButton::Left) => {
                self.param_base.begin_set_parameter(cx);
                self.param_base
                    .set_normalized_value(cx, self.normalized_value);
                self.param_base.end_set_parameter(cx);

                meta.consume();
            }
            _ => {}
        });
    }
}

/// A row of [`SegmentButton`]s, one per step of a stepped parameter, named by `labels`.
pub fn segmented<L, Params, P, FMap>(
    cx: &mut Context,
    params: L,
    params_to_param: FMap,
    labels: &'static [&'static str],
    accent: &'static str,
) where
    L: Lens<Target = Params> + Clone,
    Params: 'static,
    P: Param + 'static,
    FMap: Fn(&Params) -> &P + Copy + 'static,
{
    let last = (labels.len() - 1).max(1) as f32;
    HStack::new(cx, move |cx| {
        for (idx, label) in labels.iter().enumerate() {
            SegmentButton::new(cx, params.clone(), params_to_param, idx as f32 / last, label)
                .class(accent);
        }
    })
    .class("segmented");
}

/// A band shape drawn as a small response curve, in the text colour so it inverts with a checked
/// segment.
pub struct ShapeIcon {
    shape: BandShape,
}

impl ShapeIcon {
    pub fn new(cx: &mut Context, shape: BandShape) -> Handle<'_, Self> {
        Self { shape }.build(cx, |_| ()).hoverable(false)
    }
}

impl View for ShapeIcon {
    fn element(&self) -> Option<&'static str> {
        Some("shape-icon")
    }

    fn draw(&self, cx: &mut DrawContext, canvas: &mut Canvas) {
        let bounds = cx.bounds();
        if bounds.w == 0.0 || bounds.h == 0.0 {
            return;
        }

        // Drawn on a 24x16 grid, scaled into the bounds
        let (sx, sy) = (bounds.w / 24.0, bounds.h / 16.0);
        let point = |x: f32, y: f32| (bounds.x + x * sx, bounds.y + y * sy);
        let move_to = |path: &mut vg::Path, x: f32, y: f32| {
            let (x, y) = point(x, y);
            path.move_to(x, y);
        };
        let line_to = |path: &mut vg::Path, x: f32, y: f32| {
            let (x, y) = point(x, y);
            path.line_to(x, y);
        };
        let curve_to = |path: &mut vg::Path, c: [f32; 6]| {
            let (c1x, c1y) = point(c[0], c[1]);
            let (c2x, c2y) = point(c[2], c[3]);
            let (x, y) = point(c[4], c[5]);
            path.bezier_to(c1x, c1y, c2x, c2y, x, y);
        };

        let mut path = vg::Path::new();
        match self.shape {
            BandShape::LowCut => {
                move_to(&mut path, 3.0, 14.0);
                curve_to(&mut path, [8.0, 14.0, 9.0, 3.0, 13.0, 3.0]);
                line_to(&mut path, 22.0, 3.0);
            }
            BandShape::LowShelf => {
                move_to(&mut path, 2.0, 4.0);
                line_to(&mut path, 8.0, 4.0);
                curve_to(&mut path, [12.0, 4.0, 12.0, 11.0, 16.0, 11.0]);
                line_to(&mut path, 22.0, 11.0);
            }
            BandShape::Bell => {
                move_to(&mut path, 2.0, 12.0);
                line_to(&mut path, 7.0, 12.0);
                curve_to(&mut path, [10.0, 12.0, 10.0, 3.0, 12.0, 3.0]);
                curve_to(&mut path, [14.0, 3.0, 14.0, 12.0, 17.0, 12.0]);
                line_to(&mut path, 22.0, 12.0);
            }
            BandShape::HighShelf => {
                move_to(&mut path, 2.0, 11.0);
                line_to(&mut path, 8.0, 11.0);
                curve_to(&mut path, [12.0, 11.0, 12.0, 4.0, 16.0, 4.0]);
                line_to(&mut path, 22.0, 4.0);
            }
            BandShape::HighCut => {
                move_to(&mut path, 2.0, 3.0);
                line_to(&mut path, 11.0, 3.0);
                curve_to(&mut path, [15.0, 3.0, 16.0, 14.0, 21.0, 14.0]);
            }
        }

        let color: vg::Color = cx.font_color().into();
        canvas.stroke_path(
            &path,
            &vg::Paint::color(color).with_line_width(1.5 * cx.scale_factor()),
        );
    }
}

/// The named resolutions: `(name, window size order, overlap order)`. Finer resolution separates
/// resonances more cleanly at the cost of latency and time smearing.
const RESOLUTIONS: [(&str, i32, i32); 4] = [
    ("FAST", 10, 3),
    ("BALANCED", 11, 4),
    ("FINE", 12, 4),
    ("ULTRA", 13, 5),
];

/// One button for the STFT settings. A click steps to the next named resolution, a right-click to
/// the previous one. Settings that match none of them (from automation, say) read as CUSTOM.
pub struct ResolutionButton {
    window_size_order: ParamWidgetBase,
    overlap_times_order: ParamWidgetBase,
}

impl ResolutionButton {
    pub fn new(cx: &mut Context) -> Handle<'_, Self> {
        let name = Data::params.map(|p| {
            let current = (
                p.global.window_size_order.value(),
                p.global.overlap_times_order.value(),
            );
            RESOLUTIONS
                .iter()
                .find(|(_, window, overlap)| (*window, *overlap) == current)
                .map(|(name, _, _)| *name)
                .unwrap_or("CUSTOM")
        });

        Self {
            window_size_order: ParamWidgetBase::new(cx, Data::params, |p| {
                &p.global.window_size_order
            }),
            overlap_times_order: ParamWidgetBase::new(cx, Data::params, |p| {
                &p.global.overlap_times_order
            }),
        }
        .build(cx, move |cx| {
            Label::new(cx, name).class("segment-label").hoverable(false);
        })
    }

    fn step(&self, cx: &mut EventContext, forwards: bool) {
        let current = (
            self.window_size_order.unmodulated_plain_value() as i32,
            self.overlap_times_order.unmodulated_plain_value() as i32,
        );
        let count = RESOLUTIONS.len();
        let idx = match RESOLUTIONS
            .iter()
            .position(|(_, window, overlap)| (*window, *overlap) == current)
        {
            Some(idx) if forwards => (idx + 1) % count,
            Some(idx) => (idx + count - 1) % count,
            // From a custom setting, go back to the default
            None => 1,
        };

        let (_, window, overlap) = RESOLUTIONS[idx];
        for (param, plain) in [
            (&self.window_size_order, window),
            (&self.overlap_times_order, overlap),
        ] {
            param.begin_set_parameter(cx);
            param.set_normalized_value(cx, param.preview_normalized(plain as f32));
            param.end_set_parameter(cx);
        }
    }
}

impl View for ResolutionButton {
    fn element(&self) -> Option<&'static str> {
        Some("resolution-button")
    }

    fn event(&mut self, cx: &mut EventContext, event: &mut Event) {
        event.map(|window_event, meta| match window_event {
            WindowEvent::MouseDown(MouseButton::Left) => {
                self.step(cx, true);
                meta.consume();
            }
            WindowEvent::MouseDown(MouseButton::Right) => {
                self.step(cx, false);
                meta.consume();
            }
            _ => {}
        });
    }
}
