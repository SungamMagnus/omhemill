// Ómhemill: a rotary knob widget for nih-plug/Vizia parameters, in the same
// visual language as the Sungam web prototype's hand-drawn SVG knobs. Built
// against nih_plug_vizia's ParamWidgetBase, the same integration point
// ParamSlider uses, so automation, host modulation display and text-entry
// gestures all behave the same way a stock widget's would.

use nih_plug::prelude::Param;
use nih_plug_vizia::vizia::prelude::*;
use nih_plug_vizia::vizia::vg;
use nih_plug_vizia::widgets::param_base::ParamWidgetBase;
use nih_plug_vizia::widgets::util::ModifiersExt;
use std::f32::consts::PI;

/// One pixel dragged corresponds to this much change in the normalized value in the regular
/// (non-granular) case. Shift-dragging divides this by [`GRANULAR_DRAG_MULTIPLIER`].
const DRAG_SENSITIVITY_PX: f32 = 140.0;
const GRANULAR_DRAG_MULTIPLIER: f32 = 4.0;

const START_ANGLE: f32 = -225.0 * PI / 180.0;
const END_ANGLE: f32 = 45.0 * PI / 180.0;

#[derive(Debug, Clone, Copy)]
struct DragStatus {
    starting_y: f32,
    starting_value: f32,
}

/// A rotary knob bound to a NIH-plug parameter. Drag vertically to change the value, shift-drag
/// for finer control, scroll to nudge, and double-click (or right-click, or command-click) to
/// reset to the default.
pub struct Knob {
    param_base: ParamWidgetBase,
    drag_active: bool,
    drag_status: Option<DragStatus>,
    scrolled_lines: f32,
    /// The color used for the value arc and pointer. Distinguishes each bank the way the web
    /// prototype's per-section knob colors do.
    accent: Color,
}

impl Knob {
    pub fn new<L, Params, P, FMap>(
        cx: &mut Context,
        params: L,
        params_to_param: FMap,
        accent: Color,
    ) -> Handle<Self>
    where
        L: Lens<Target = Params> + Clone,
        Params: 'static,
        P: Param + 'static,
        FMap: Fn(&Params) -> &P + Copy + 'static,
    {
        Self {
            param_base: ParamWidgetBase::new(cx, params, params_to_param),
            drag_active: false,
            drag_status: None,
            scrolled_lines: 0.0,
            accent,
        }
        .build(cx, |_cx| ())
    }

    fn reset_to_default(&self, cx: &mut EventContext) {
        self.param_base.begin_set_parameter(cx);
        self.param_base
            .set_normalized_value(cx, self.param_base.default_normalized_value());
        self.param_base.end_set_parameter(cx);
    }
}

impl View for Knob {
    fn element(&self) -> Option<&'static str> {
        Some("knob-dial")
    }

    fn event(&mut self, cx: &mut EventContext, event: &mut Event) {
        event.map(|window_event, meta| match window_event {
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
                    self.drag_status = Some(DragStatus {
                        starting_y: cx.mouse().cursory,
                        starting_value: self.param_base.unmodulated_normalized_value(),
                    });
                }

                meta.consume();
            }
            WindowEvent::MouseDoubleClick(MouseButton::Left)
            | WindowEvent::MouseDown(MouseButton::Right)
            | WindowEvent::MouseDoubleClick(MouseButton::Right) => {
                self.reset_to_default(cx);
                meta.consume();
            }
            WindowEvent::MouseUp(MouseButton::Left) => {
                if self.drag_active {
                    self.drag_active = false;
                    self.drag_status = None;
                    cx.release();
                    cx.set_active(false);

                    self.param_base.end_set_parameter(cx);

                    meta.consume();
                }
            }
            WindowEvent::MouseMove(_x, y) => {
                if self.drag_active {
                    if let Some(status) = self.drag_status {
                        let sensitivity = if cx.modifiers().shift() {
                            DRAG_SENSITIVITY_PX * GRANULAR_DRAG_MULTIPLIER
                        } else {
                            DRAG_SENSITIVITY_PX
                        } * cx.scale_factor();

                        let delta = (status.starting_y - *y) / sensitivity;
                        self.param_base
                            .set_normalized_value(cx, (status.starting_value + delta).clamp(0.0, 1.0));
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
        let center_x = bounds.x + bounds.w / 2.0;
        let center_y = bounds.y + bounds.h / 2.0;
        let radius = (bounds.w.min(bounds.h) / 2.0) - (3.0 * scale);
        let line_width = 2.6 * scale;

        let value = self.param_base.unmodulated_normalized_value();
        let value_angle = START_ANGLE + (END_ANGLE - START_ANGLE) * value;

        let border_color: vg::Color = cx.border_color().into();
        let background_color: vg::Color = cx.background_color().into();
        let track_color = vg::Color::rgbaf(
            border_color.r,
            border_color.g,
            border_color.b,
            0.22,
        );

        // Face
        let mut face = vg::Path::new();
        face.arc(center_x, center_y, radius * 0.62, 0.0, PI * 2.0, vg::Solidity::Solid);
        canvas.fill_path(&face, &vg::Paint::color(background_color));
        canvas.stroke_path(
            &face,
            &vg::Paint::color(border_color).with_line_width(1.4 * scale),
        );

        // Background track
        let mut track = vg::Path::new();
        track.arc(center_x, center_y, radius, START_ANGLE, END_ANGLE, vg::Solidity::Hole);
        canvas.stroke_path(
            &track,
            &vg::Paint::color(track_color)
                .with_line_width(line_width)
                .with_line_cap(vg::LineCap::Round),
        );

        // Value arc
        if value > 0.001 {
            let mut fill = vg::Path::new();
            fill.arc(center_x, center_y, radius, START_ANGLE, value_angle, vg::Solidity::Hole);
            let accent: vg::Color = self.accent.into();
            canvas.stroke_path(
                &fill,
                &vg::Paint::color(accent)
                    .with_line_width(line_width)
                    .with_line_cap(vg::LineCap::Round),
            );
        }

        // Pointer
        let inner = radius * 0.30;
        let outer = radius - (2.0 * scale);
        let (lx, ly) = (
            center_x + value_angle.cos() * inner,
            center_y + value_angle.sin() * inner,
        );
        let (mx, my) = (
            center_x + value_angle.cos() * outer,
            center_y + value_angle.sin() * outer,
        );
        let mut pointer = vg::Path::new();
        pointer.move_to(lx, ly);
        pointer.line_to(mx, my);
        canvas.stroke_path(
            &pointer,
            &vg::Paint::color(border_color)
                .with_line_width(1.8 * scale)
                .with_line_cap(vg::LineCap::Round),
        );
    }
}

/// A [`Knob`] with a live value readout and a static name label beneath it, matching the
/// composition of the web prototype's knob widgets.
pub fn labeled_knob<L, Params, P, FMap>(
    cx: &mut Context,
    params: L,
    params_to_param: FMap,
    label: &str,
    accent: Color,
) where
    L: Lens<Target = Params> + Clone,
    Params: 'static,
    P: Param + 'static,
    FMap: Fn(&Params) -> &P + Copy + 'static,
{
    VStack::new(cx, |cx| {
        Knob::new(cx, params, params_to_param, accent)
            .width(Pixels(46.0))
            .height(Pixels(46.0))
            .left(Stretch(1.0))
            .right(Stretch(1.0));

        ParamWidgetBase::view(cx, params, params_to_param, |cx, param_data| {
            let display_value_lens = param_data.make_lens(|param| {
                param.normalized_value_to_string(param.unmodulated_normalized_value(), true)
            });
            Label::new(cx, display_value_lens)
                .class("knob-value")
                .left(Stretch(1.0))
                .right(Stretch(1.0));
        });

        Label::new(cx, label)
            .class("knob-name")
            .left(Stretch(1.0))
            .right(Stretch(1.0));
    })
    .class("knob-cell")
    .width(Pixels(84.0))
    .height(Auto)
    .row_between(Pixels(3.0));
}
