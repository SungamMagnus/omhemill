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
use nih_plug_vizia::{assets, create_vizia_editor, ViziaState, ViziaTheming};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

use self::analyzer::Analyzer;
use self::knob::labeled_knob;
use crate::analyzer::AnalyzerData;
use crate::curve::NUM_NODES;
use crate::OmhemillParams;

mod analyzer;
mod knob;

/// The plugin window's size, in logical pixels. This mirrors the proportions of the design.
const GUI_WIDTH: u32 = 1160;
const GUI_HEIGHT: u32 = 700;

/// The height of the spectrum display.
const DISPLAY_HEIGHT: f32 = 330.0;

// Per-bank accents, matching the Sungam palette used by the web prototype.
const DETECT: Color = Color::rgb(0xc2, 0x52, 0x1a);
const SHAPE: Color = Color::rgb(0x2e, 0x4b, 0x8f);
const TIME: Color = Color::rgb(0x0f, 0x7a, 0x72);
const SPECTRUM: Color = Color::rgb(0xa8, 0x28, 0x5a);
const OUTPUT: Color = Color::rgb(0x19, 0x1c, 0x1b);

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
        assets::register_noto_sans_light(cx);
        assets::register_noto_sans_thin(cx);
        assets::register_noto_sans_regular(cx);

        if let Err(err) = cx.add_stylesheet(include_style!("src/editor/theme.css")) {
            nih_error!("Failed to load stylesheet: {err:?}")
        }

        editor_data.clone().build(cx);

        VStack::new(cx, |cx| {
            header(cx);
            Element::new(cx).class("divider");
            display(cx);
            node_strip(cx);
            Element::new(cx).class("divider");
            banks(cx);
            Element::new(cx).class("divider");
            footer(cx);
        })
        .class("frame");

        ResizeHandle::new(cx);
    })
}

/// The wordmark, the tagline, and the threshold mode on the right.
fn header(cx: &mut Context) {
    HStack::new(cx, |cx| {
        Label::new(cx, "ÓMHEMILL")
            .font_family(vec![FamilyOwned::Name(String::from(assets::NOTO_SANS))])
            .font_weight(FontWeightKeyword::Thin)
            .class("wordmark");

        Label::new(cx, "spectral resonance suppressor").class("tagline");

        // Pushes the mode control over to the right
        Element::new(cx).width(Stretch(1.0)).height(Pixels(0.0));

        Label::new(cx, "MODE").class("strip-label");
        ParamSlider::new(cx, Data::params, |p| &p.threshold.mode)
            .set_style(ParamSliderStyle::CurrentStepLabeled { even: true })
            .class("mode-slider");
    })
    .class("header")
    .height(Pixels(36.0))
    .col_between(Pixels(12.0));
}

/// The spectrum display. The analyzer view draws its own grid, axis labels, spectrum, threshold
/// curve, gain reduction and node handles.
fn display(cx: &mut Context) {
    Analyzer::new(cx, Data::analyzer_data, Data::sample_rate, Data::params)
        .class("display")
        .width(Stretch(1.0))
        .height(Pixels(DISPLAY_HEIGHT));
}

/// One cell per node: a numbered button that switches the node on and off, plus its current
/// frequency and depth.
fn node_strip(cx: &mut Context) {
    HStack::new(cx, |cx| {
        Label::new(cx, "NODES").class("strip-label");

        for idx in 0..NUM_NODES {
            VStack::new(cx, move |cx| {
                HStack::new(cx, move |cx| {
                    ParamButton::new(cx, Data::params, move |p| &p.threshold.nodes[idx].enabled)
                        .with_label(format!("{}", idx + 1).as_str())
                        .class("node-dot");

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
                    .class("node-freq");
                })
                .height(Auto)
                .col_between(Pixels(7.0));

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
                .class("node-depth");
            })
            .class("node-cell")
            .width(Stretch(1.0))
            .height(Auto)
            .row_between(Pixels(2.0));
        }
    })
    .class("strip")
    .height(Auto)
    .col_between(Pixels(10.0));
}

/// The five knob banks.
fn banks(cx: &mut Context) {
    HStack::new(cx, |cx| {
        bank(cx, "DETECT", "skynjun", "rule-detect", |cx| {
            labeled_knob(
                cx,
                Data::params,
                |p| &p.threshold.threshold_db,
                "Depth",
                DETECT,
            );
            labeled_knob(
                cx,
                Data::params,
                |p| &p.compressors.downwards.ratio,
                "Sharpness",
                DETECT,
            );
            labeled_knob(
                cx,
                Data::params,
                |p| &p.compressors.downwards.knee_width_db,
                "Selectivity",
                DETECT,
            );
        });

        bank(cx, "SHAPE", "form", "rule-shape", |cx| {
            labeled_knob(cx, Data::params, |p| &p.threshold.curve_slope, "Tilt", SHAPE);
            labeled_knob(cx, Data::params, |p| &p.threshold.curve_curve, "Bend", SHAPE);
            labeled_knob(
                cx,
                Data::params,
                |p| &p.threshold.center_frequency,
                "Center",
                SHAPE,
            );
        });

        bank(cx, "TIME", "tími", "rule-time", |cx| {
            labeled_knob(
                cx,
                Data::params,
                |p| &p.global.compressor_attack_ms,
                "Attack",
                TIME,
            );
            labeled_knob(
                cx,
                Data::params,
                |p| &p.global.compressor_release_ms,
                "Release",
                TIME,
            );
        });

        bank(cx, "SPECTRUM", "róf", "rule-spectrum", |cx| {
            labeled_knob(
                cx,
                Data::params,
                |p| &p.global.window_size_order,
                "Resolution",
                SPECTRUM,
            );
            labeled_knob(
                cx,
                Data::params,
                |p| &p.global.overlap_times_order,
                "Overlap",
                SPECTRUM,
            );
        });

        bank(cx, "OUTPUT", "út", "rule-output", |cx| {
            labeled_knob(cx, Data::params, |p| &p.global.dry_wet_ratio, "Mix", OUTPUT);
            labeled_knob(cx, Data::params, |p| &p.global.output_gain, "Output", OUTPUT);
        });
    })
    .class("banks")
    .height(Auto)
    .col_between(Pixels(16.0));
}

/// A single bank: a coloured rule, the English name with its Icelandic counterpart, and a row of
/// knobs.
fn bank(
    cx: &mut Context,
    title: &str,
    subtitle: &str,
    rule_class: &str,
    contents: impl FnOnce(&mut Context),
) {
    VStack::new(cx, |cx| {
        Element::new(cx).class("bank-rule").class(rule_class);

        HStack::new(cx, |cx| {
            Label::new(cx, title).class("bank-title").class(rule_class);
            Element::new(cx).width(Stretch(1.0)).height(Pixels(0.0));
            Label::new(cx, subtitle).class("bank-sub");
        })
        .height(Auto);

        HStack::new(cx, contents)
            .height(Auto)
            .top(Pixels(6.0))
            .col_between(Pixels(2.0));
    })
    .class("bank")
    .width(Stretch(1.0))
    .height(Auto)
    .row_between(Pixels(6.0));
}

/// What the STFT is actually doing, so the cost of the current settings is visible.
fn footer(cx: &mut Context) {
    HStack::new(cx, |cx| {
        Label::new(cx, "BANDS").class("footer-label");
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

        Label::new(cx, "OVERLAP").class("footer-label");
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

        Label::new(cx, "ÓMHEMILL").class("footer-label");
        Label::new(cx, crate::Omhemill::VERSION).class("footer-value");
    })
    .class("footer")
    .height(Auto)
    .col_between(Pixels(8.0));
}
