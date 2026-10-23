// Ómhemill: a spectral resonance suppressor.
// Copyright (C) 2026 Sungam
//
// Built on Spectral Compressor
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
use crossbeam::atomic::AtomicCell;
use nih_plug::prelude::*;
use nih_plug_vizia::vizia::prelude::*;
use nih_plug_vizia::widgets::param_base::ParamWidgetBase;
use nih_plug_vizia::widgets::*;
use nih_plug_vizia::{create_vizia_editor, ViziaState, ViziaTheming};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

use self::analyzer::Analyzer;
use self::controls::{labeled_bar, segmented, BarSlider};
use crate::analyzer::AnalyzerData;
use crate::curve::NUM_NODES;
use crate::OmhemillParams;

mod analyzer;
mod controls;

/// The plugin window's size, in logical pixels. This mirrors the proportions of the design.
const GUI_WIDTH: u32 = 1200;
const GUI_HEIGHT: u32 = 645;

/// Kept so existing saved state still loads. The editor no longer has a collapsed mode — the
/// display is always shown, as in the design.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EditorMode {
    #[serde(rename = "collapsed")]
    Collapsed,
    #[default]
    #[serde(rename = "analyzer-visible")]
    AnalyzerVisible,
}

#[derive(Clone, Lens)]
pub struct Data {
    pub(crate) params: Arc<OmhemillParams>,
    pub(crate) editor_mode: Arc<AtomicCell<EditorMode>>,
    pub(crate) analyzer_data: Arc<Mutex<triple_buffer::Output<AnalyzerData>>>,
    /// Used by the analyzer to determine which FFT bins belong to which frequencies.
    pub(crate) sample_rate: Arc<AtomicF32>,
}

impl Model for Data {}

pub(crate) fn default_state(_editor_mode: Arc<AtomicCell<EditorMode>>) -> Arc<ViziaState> {
    ViziaState::new(move || (GUI_WIDTH, GUI_HEIGHT))
}

pub(crate) fn create(editor_state: Arc<ViziaState>, editor_data: Data) -> Option<Box<dyn Editor>> {
    create_vizia_editor(editor_state, ViziaTheming::Custom, move |cx, _| {
        if let Err(err) = cx.add_stylesheet(include_style!("src/editor/theme.css")) {
            nih_error!("Failed to load stylesheet: {err:?}")
        }

        editor_data.clone().build(cx);

        VStack::new(cx, |cx| {
            header(cx);
            Element::new(cx).class("divider");
            bands(cx);
            banks(cx);
            Element::new(cx).class("divider");
            footer(cx);
        })
        .class("frame-root");

        ResizeHandle::new(cx);
    })
}

/// A section drawn as the box its controls sit inside: a hairline in the section colour, with the
/// title inset into the top rule. `accent` is the stylesheet class for the section colour.
fn frame<'a>(
    cx: &'a mut Context,
    title: &str,
    accent: &'static str,
    contents: impl FnOnce(&mut Context),
) -> Handle<'a, VStack> {
    VStack::new(cx, |cx| {
        contents(cx);

        // Built last so it paints over the frame's own border
        Label::new(cx, title).class("frame-title").class(accent);
    })
    .class("frame")
    .class(accent)
}

/// The wordmark, then what the brake listens to, then the STFT settings on the right.
fn header(cx: &mut Context) {
    HStack::new(cx, |cx| {
        HStack::new(cx, |cx| {
            for letter in ["Ó", "M", "H", "E", "M", "I", "L", "L"] {
                Label::new(cx, letter);
            }
        })
        .class("wordmark");

        VStack::new(cx, |cx| {
            Label::new(cx, "LISTEN TO").class("cap");
            segmented(
                cx,
                Data::params,
                |p| &p.threshold.mode,
                &["SELF", "SIDECHAIN", "DUCK"],
                "violet",
            );
        })
        .class("field");

        // Pushes the STFT settings over to the right
        Element::new(cx).width(Stretch(1.0)).height(Pixels(0.0));

        VStack::new(cx, |cx| {
            Label::new(cx, "RESOLUTION").class("cap");
            BarSlider::new(cx, Data::params, |p| &p.global.window_size_order)
                .class("ink")
                .width(Pixels(132.0));
        })
        .class("field");

        VStack::new(cx, |cx| {
            Label::new(cx, "OVERLAP").class("cap");
            BarSlider::new(cx, Data::params, |p| &p.global.overlap_times_order)
                .class("ink")
                .width(Pixels(84.0));
        })
        .class("field");
    })
    .class("header");
}

/// The spectrum display and the row of band chips under it, in one coral frame. The analyzer
/// draws its own grid, axis labels, spectrum, threshold curve, gain reduction and band handles.
fn bands(cx: &mut Context) {
    VStack::new(cx, |cx| {
        frame(cx, "BANDS", "coral", |cx| {
            Analyzer::new(cx, Data::analyzer_data, Data::sample_rate, Data::params)
                .width(Stretch(1.0))
                .height(Stretch(1.0));

            band_chips(cx);
        })
        .height(Stretch(1.0));
    })
    .child_left(Pixels(32.0))
    .child_right(Pixels(32.0))
    .child_top(Pixels(22.0))
    .height(Stretch(1.0));
}

/// One chip per band: a numbered button that switches the band on and off, plus its current
/// frequency and depth.
fn band_chips(cx: &mut Context) {
    HStack::new(cx, |cx| {
        for idx in 0..NUM_NODES {
            HStack::new(cx, move |cx| {
                ParamButton::new(cx, Data::params, move |p| &p.threshold.nodes[idx].enabled)
                    .with_label(format!("{}", idx + 1).as_str())
                    .class("chip-dot");

                Label::new(
                    cx,
                    ParamWidgetBase::make_lens(
                        Data::params,
                        move |p| &p.threshold.nodes[idx].frequency,
                        |param| {
                            param.normalized_value_to_string(
                                param.unmodulated_normalized_value(),
                                true,
                            )
                        },
                    ),
                )
                .class("chip-freq");

                Label::new(
                    cx,
                    ParamWidgetBase::make_lens(
                        Data::params,
                        move |p| &p.threshold.nodes[idx].depth_db,
                        |param| {
                            param.normalized_value_to_string(
                                param.unmodulated_normalized_value(),
                                true,
                            )
                        },
                    ),
                )
                .class("chip-depth");
            })
            .class("chip")
            .width(Stretch(1.0));
        }
    })
    .class("chips");
}

/// The global controls, one frame per signal colour.
fn banks(cx: &mut Context) {
    HStack::new(cx, |cx| {
        frame(cx, "DETECT", "coral", |cx| {
            HStack::new(cx, |cx| {
                labeled_bar(cx, Data::params, |p| &p.threshold.threshold_db, "DEPTH", "coral");
                labeled_bar(
                    cx,
                    Data::params,
                    |p| &p.compressors.downwards.ratio,
                    "SHARPNESS",
                    "coral",
                );
                labeled_bar(
                    cx,
                    Data::params,
                    |p| &p.compressors.downwards.knee_width_db,
                    "SELECTIVITY",
                    "coral",
                );
            })
            .class("bars");
        })
        .width(Stretch(3.0))
        .height(Auto);

        // Tilt, bend and center stay until the bands can do their job (shelves, wide bells)
        frame(cx, "CURVE", "coral", |cx| {
            HStack::new(cx, |cx| {
                labeled_bar(cx, Data::params, |p| &p.threshold.curve_slope, "TILT", "coral");
                labeled_bar(cx, Data::params, |p| &p.threshold.curve_curve, "BEND", "coral");
                labeled_bar(
                    cx,
                    Data::params,
                    |p| &p.threshold.center_frequency,
                    "CENTER",
                    "coral",
                );
            })
            .class("bars");
        })
        .width(Stretch(3.0))
        .height(Auto);

        frame(cx, "TIME", "teal", |cx| {
            HStack::new(cx, |cx| {
                labeled_bar(
                    cx,
                    Data::params,
                    |p| &p.global.compressor_attack_ms,
                    "ATTACK",
                    "teal",
                );
                labeled_bar(
                    cx,
                    Data::params,
                    |p| &p.global.compressor_release_ms,
                    "RELEASE",
                    "teal",
                );
            })
            .class("bars");
        })
        .width(Stretch(2.0))
        .height(Auto);

        frame(cx, "OUTPUT", "steel", |cx| {
            HStack::new(cx, |cx| {
                labeled_bar(cx, Data::params, |p| &p.global.dry_wet_ratio, "MIX", "steel");
                labeled_bar(cx, Data::params, |p| &p.global.output_gain, "TRIM", "steel");
            })
            .class("bars");
        })
        .width(Stretch(2.0))
        .height(Auto);
    })
    .class("banks")
    .child_left(Pixels(32.0))
    .child_right(Pixels(32.0))
    .child_top(Pixels(24.0))
    .child_bottom(Pixels(14.0));
}

/// What the STFT is actually doing, so the cost of the current settings is visible.
fn footer(cx: &mut Context) {
    HStack::new(cx, |cx| {
        Label::new(cx, "FFT");
        Label::new(
            cx,
            ParamWidgetBase::make_lens(
                Data::params,
                |p| &p.global.window_size_order,
                |param| {
                    param.normalized_value_to_string(param.unmodulated_normalized_value(), true)
                },
            ),
        )
        .class("footer-value");

        Label::new(cx, "OVERLAP");
        Label::new(
            cx,
            ParamWidgetBase::make_lens(
                Data::params,
                |p| &p.global.overlap_times_order,
                |param| {
                    param.normalized_value_to_string(param.unmodulated_normalized_value(), true)
                },
            ),
        )
        .class("footer-value");

        Element::new(cx).width(Stretch(1.0)).height(Pixels(0.0));

        Label::new(cx, "ÓMHEMILL");
        Label::new(cx, crate::Omhemill::VERSION).class("footer-value");
    })
    .class("footer");
}
