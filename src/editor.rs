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
use nih_plug_vizia::widgets::*;
use nih_plug_vizia::{create_vizia_editor, ViziaState, ViziaTheming};
use serde::{Deserialize, Serialize};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use self::analyzer::Analyzer;
use self::controls::{labeled_bar, segmented, ResolutionButton, SegmentButton, ShapeIcon};
use crate::analyzer::AnalyzerData;
use crate::curve::{BandShape, Curve, NUM_BANDS};
use crate::presets::{self, Preset};
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
    /// The band whose panel is open, if any. Shared by the display and the chips.
    pub(crate) selected_band: Option<usize>,
    /// The band being soloed, if any. Mirrored into [`OmhemillParams::solo_band`] for the audio
    /// thread.
    pub(crate) solo_band: Option<usize>,
    /// The factory presets followed by the user's own. Loaded when the editor opens.
    pub(crate) presets: Vec<Preset>,
    /// The preset last loaded or saved, if any.
    pub(crate) preset_index: Option<usize>,
    /// The name of the preset last loaded or saved, shown on the preset menu.
    pub(crate) preset_name: String,
    /// Whether the preset menu has turned into a name field for saving.
    pub(crate) saving: bool,
    /// What the name field holds while saving.
    pub(crate) save_name: String,
}

/// Selecting a band opens its panel; selecting nothing closes it.
pub enum BandSelection {
    Select(Option<usize>),
}

/// Soloing a band plays only its range.
pub enum SoloEvent {
    /// Solo the band, or stop soloing it if it already is.
    Toggle(usize),
    Set(Option<usize>),
}

pub enum PresetEvent {
    Previous,
    Next,
    Load(usize),
    /// Turn the preset menu into a name field or, if it already is one, save under that name.
    Save,
    /// The name field was edited.
    EditSaveName(String),
    /// Leave the name field without saving.
    CancelSave,
}

impl Data {
    fn set_preset_name(&mut self, name: String) {
        *self.params.preset_name.lock().unwrap() = name.clone();
        self.preset_name = name;
    }

    fn set_solo(&mut self, solo_band: Option<usize>) {
        self.solo_band = solo_band;
        self.params.solo_band.store(
            solo_band.map(|idx| idx as i32).unwrap_or(-1),
            Ordering::Relaxed,
        );
    }

    /// Load a preset: every parameter it mentions, and every other one back to its default.
    fn load_preset(&mut self, cx: &mut EventContext, idx: usize) {
        let Some(preset) = self.presets.get(idx) else {
            return;
        };

        for (ptr, value) in presets::values_to_apply(&self.params, preset) {
            cx.emit(RawParamEvent::BeginSetParameter(ptr));
            cx.emit(RawParamEvent::SetParameterNormalized(ptr, value));
            cx.emit(RawParamEvent::EndSetParameter(ptr));
        }

        self.preset_index = Some(idx);
        let name = preset.name.clone();
        self.set_preset_name(name);
        self.selected_band = None;
        self.set_solo(None);
    }

    /// Save the current state as a user preset under the name field's name. A factory preset's
    /// name gets " copy" added so the factory set stays as it ships.
    fn save_preset(&mut self) {
        self.saving = false;
        let mut name = self.save_name.trim().to_owned();
        if name.is_empty() {
            name = String::from("Untitled");
        }
        if self.presets.iter().any(|preset| preset.factory && preset.name == name) {
            name.push_str(" copy");
        }

        let preset = presets::capture(&self.params, &name);
        if let Err(err) = presets::save_user(&preset) {
            nih_error!("Could not save the preset {name:?}: {err}");
            return;
        }

        self.presets = presets::all(&self.params);
        self.preset_index = self
            .presets
            .iter()
            .position(|preset| !preset.factory && preset.name == name);
        self.set_preset_name(name);
    }
}

impl Model for Data {
    fn event(&mut self, cx: &mut EventContext, event: &mut Event) {
        event.map(|selection, _| match selection {
            BandSelection::Select(selected_band) => self.selected_band = *selected_band,
        });

        event.map(|solo, _| match solo {
            SoloEvent::Toggle(idx) => {
                let solo_band = if self.solo_band == Some(*idx) {
                    None
                } else {
                    Some(*idx)
                };
                self.set_solo(solo_band);
            }
            SoloEvent::Set(solo_band) => self.set_solo(*solo_band),
        });

        event.map(|preset_event, _| match preset_event {
            PresetEvent::Previous | PresetEvent::Next if !self.presets.is_empty() => {
                let count = self.presets.len();
                let idx = match (self.preset_index, preset_event) {
                    (Some(idx), PresetEvent::Previous) => (idx + count - 1) % count,
                    (Some(idx), _) => (idx + 1) % count,
                    (None, _) => 0,
                };
                self.load_preset(cx, idx);
            }
            PresetEvent::Previous | PresetEvent::Next => (),
            PresetEvent::Load(idx) => self.load_preset(cx, *idx),
            PresetEvent::Save if self.saving => self.save_preset(),
            PresetEvent::Save => {
                self.save_name = self.preset_name.clone();
                self.saving = true;
            }
            PresetEvent::EditSaveName(name) => self.save_name = name.clone(),
            PresetEvent::CancelSave => self.saving = false,
        });
    }
}

pub(crate) fn default_state(_editor_mode: Arc<AtomicCell<EditorMode>>) -> Arc<ViziaState> {
    ViziaState::new(move || (GUI_WIDTH, GUI_HEIGHT))
}

pub(crate) fn create(editor_state: Arc<ViziaState>, editor_data: Data) -> Option<Box<dyn Editor>> {
    create_vizia_editor(editor_state, ViziaTheming::Custom, move |cx, _| {
        if let Err(err) = cx.add_stylesheet(include_style!("src/editor/theme.css")) {
            nih_error!("Failed to load stylesheet: {err:?}")
        }

        let mut data = editor_data.clone();
        data.presets = presets::all(&data.params);
        data.preset_index = data
            .presets
            .iter()
            .position(|preset| preset.name == data.preset_name);
        data.build(cx);

        VStack::new(cx, |cx| {
            header(cx);
            Element::new(cx).class("divider");
            bands(cx);
            banks(cx);
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

/// A caption over a header control.
fn field(cx: &mut Context, caption: &str, contents: impl FnOnce(&mut Context)) {
    VStack::new(cx, |cx| {
        Label::new(cx, caption).class("cap");
        contents(cx);
    })
    .class("field");
}

/// The wordmark, the presets, the compression mode and the sidechain, then resolution and the
/// two monitoring switches on the right. Every control is the same height as a segment.
fn header(cx: &mut Context) {
    HStack::new(cx, |cx| {
        HStack::new(cx, |cx| {
            for letter in ["Ó", "M", "H", "E", "M", "I", "L", "L"] {
                Label::new(cx, letter);
            }
        })
        .class("wordmark");

        field(cx, "PRESET", |cx| {
            HStack::new(cx, |cx| {
                Button::new(cx, |cx| cx.emit(PresetEvent::Previous), |cx| Label::new(cx, "<"))
                    .class("header-button")
                    .class("preset-step");

                // The menu, or while saving, a field for the new preset's name
                Binding::new(cx, Data::saving, |cx, saving| {
                    if saving.get(cx) {
                        Textbox::new(cx, Data::save_name)
                            .on_edit(|cx, name| cx.emit(PresetEvent::EditSaveName(name)))
                            .on_submit(|cx, name, pressed_enter| {
                                cx.emit(PresetEvent::EditSaveName(name));
                                if pressed_enter {
                                    cx.emit(PresetEvent::Save);
                                }
                            })
                            .on_cancel(|cx| cx.emit(PresetEvent::CancelSave))
                            .on_build(|cx| {
                                cx.emit(TextEvent::StartEdit);
                                cx.emit(TextEvent::SelectAll);
                            })
                            .class("preset-name");
                    } else {
                        preset_menu(cx);
                    }
                });

                Button::new(cx, |cx| cx.emit(PresetEvent::Next), |cx| Label::new(cx, ">"))
                    .class("header-button")
                    .class("preset-step");
                Button::new(cx, |cx| cx.emit(PresetEvent::Save), |cx| Label::new(cx, "SAVE"))
                    .class("header-button");
            })
            .class("segmented");
        });

        field(cx, "MODE", |cx| {
            segmented(
                cx,
                Data::params,
                |p| &p.threshold.action,
                &["DOWNWARD", "UPWARD", "BOTH"],
                "coral",
            );
        });

        field(cx, "SIDECHAIN", |cx| {
            segmented(
                cx,
                Data::params,
                |p| &p.threshold.mode,
                &["INT", "EXT MATCH", "EXT DUCK"],
                "violet",
            );
        });

        // Pushes the rest over to the right
        Element::new(cx).width(Stretch(1.0)).height(Pixels(0.0));

        field(cx, "RESOLUTION", |cx| {
            ResolutionButton::new(cx);
        });

        field(cx, " ", |cx| {
            HStack::new(cx, |cx| {
                ParamButton::new(cx, Data::params, |p| &p.global.delta)
                    .with_label("DELTA")
                    .class("header-toggle")
                    .class("amber");
                ParamButton::new(cx, Data::params, |p| &p.global.bypass)
                    .with_label("BYPASS")
                    .class("header-toggle");
            })
            .class("toggles");
        });
    })
    .class("header");
}

/// The preset menu: the current preset's name, which opens a list of the factory presets and
/// then the user's own, with the current one marked.
fn preset_menu(cx: &mut Context) {
    Dropdown::new(
        cx,
        |cx| {
            HStack::new(cx, |cx| {
                Label::new(cx, Data::preset_name).class("preset-current");
                Label::new(cx, "\u{25be}").class("preset-arrow");
            })
            .class("preset-button")
        },
        |cx| {
            let presets = Data::presets.get(cx);
            let current = Data::preset_index.get(cx);
            let rows = presets.len() + 3;

            ScrollView::new(cx, 0.0, 0.0, false, true, move |cx| {
                VStack::new(cx, |cx| {
                    Label::new(cx, "FACTORY").class("preset-heading");
                    for (idx, preset) in presets.iter().enumerate().filter(|(_, p)| p.factory) {
                        preset_row(cx, idx, &preset.name, current == Some(idx));
                    }

                    Label::new(cx, "USER").class("preset-heading");
                    let mut any_user = false;
                    for (idx, preset) in presets.iter().enumerate().filter(|(_, p)| !p.factory) {
                        preset_row(cx, idx, &preset.name, current == Some(idx));
                        any_user = true;
                    }
                    if !any_user {
                        Label::new(cx, "Nothing saved yet").class("preset-empty");
                    }
                })
                .class("preset-list");
            })
            // Long lists scroll rather than run off the window
            .height(Pixels((rows as f32 * 24.0 + 12.0).min(360.0)));
        },
    )
    .class("preset-menu");
}

/// One preset in the menu. Choosing it loads it and closes the menu.
fn preset_row(cx: &mut Context, idx: usize, name: &str, is_current: bool) {
    Label::new(cx, name)
        .class("preset-item")
        .toggle_class("current", is_current)
        .on_press(move |cx| {
            cx.emit(PresetEvent::Load(idx));
            cx.emit(PopupEvent::Close);
        });
}

/// The spectrum display and the row of band chips under it, in one coral frame. The analyzer
/// draws its own grid, axis labels, spectrum, sensitivity line and band handles; the selected
/// band's panel floats over it.
fn bands(cx: &mut Context) {
    VStack::new(cx, |cx| {
        frame(cx, "BANDS", "coral", |cx| {
            ZStack::new(cx, |cx| {
                Analyzer::new(cx, Data::analyzer_data, Data::sample_rate, Data::params)
                    .width(Stretch(1.0))
                    .height(Stretch(1.0));

                band_panel_overlay(cx);
            })
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

/// One chip per band: a numbered button that switches the band on and off, plus its frequency
/// and either its threshold or, for a cut, which side it fences off.
fn band_chips(cx: &mut Context) {
    HStack::new(cx, |cx| {
        for idx in 0..NUM_BANDS {
            HStack::new(cx, move |cx| {
                ParamButton::new(cx, Data::params, move |p| &p.threshold.bands[idx].enabled)
                    .with_label(format!("{}", idx + 1).as_str())
                    .class("chip-dot");

                Label::new(
                    cx,
                    Data::params.map(move |p| {
                        let freq = p.threshold.bands[idx].frequency.value();
                        if freq < 1_000.0 {
                            format!("{freq:.0} Hz")
                        } else if freq < 10_000.0 {
                            format!("{:.2}k", freq / 1_000.0)
                        } else {
                            format!("{:.1}k", freq / 1_000.0)
                        }
                    }),
                )
                .class("chip-freq");

                Label::new(
                    cx,
                    Data::params.map(move |p| {
                        let band = &p.threshold.bands[idx];
                        let threshold_db = band.threshold_db.value();
                        match band.shape.value() {
                            BandShape::LowCut => String::from("LO CUT"),
                            BandShape::HighCut => String::from("HI CUT"),
                            BandShape::LowShelf => format!("LS {threshold_db:+.1}"),
                            BandShape::HighShelf => format!("HS {threshold_db:+.1}"),
                            BandShape::Bell => format!("{threshold_db:+.1}"),
                        }
                    }),
                )
                .class("chip-depth");
            })
            .class("chip")
            .toggle_class(
                "selected",
                Data::selected_band.map(move |selected| *selected == Some(idx)),
            )
            .on_press(move |cx| cx.emit(BandSelection::Select(Some(idx))))
            .width(Stretch(1.0));
        }
    })
    .class("chips");
}

/// The selected band's panel, floating over the display. It slides along with the band: the
/// space either side of it is split in the band's position, so it stays under the band and inside
/// the display. It sits in the lower half while the band is on or above the zero line, and in the
/// upper half otherwise, so it never covers its own band.
///
/// The panel is placed directly in the display's stack rather than in a full-size row with
/// spacers: vizia skips a non-hoverable view and everything inside it when hit testing, so such a
/// row would either swallow the display's clicks or hide the panel's own.
fn band_panel_overlay(cx: &mut Context) {
    Binding::new(cx, Data::selected_band, |cx, selected_band| {
        let Some(idx) = selected_band.get(cx) else {
            return;
        };

        let position = Data::params.map(move |p| {
            let band = &p.threshold.bands[idx];
            Analyzer::freq_to_t(band.frequency.value()).clamp(0.0, 1.0)
        });
        let in_lower_half = Data::params.map(move |p| {
            let band = &p.threshold.bands[idx];
            let curve_params = p.threshold.curve_params();
            // The line shows minus the summed sensitivity, so on or above zero is at most zero
            band.shape.value().is_cut()
                || Curve::new(&curve_params).sensitivity_ln(band.frequency.value().ln()) <= 0.0
        });

        band_panel(cx, idx)
            .position_type(PositionType::SelfDirected)
            .left(position.map(|t| Stretch(*t)))
            .right(position.map(|t| Stretch(1.0 - *t)))
            .top(in_lower_half.map(|lower| if *lower { Stretch(1.0) } else { Pixels(10.0) }))
            .bottom(in_lower_half.map(|lower| if *lower { Pixels(10.0) } else { Stretch(1.0) }))
            .min_left(Pixels(10.0))
            .min_right(Pixels(10.0));
    });
}

/// The band panel: on/off, the shape, solo and close in the top row; frequency, Q and threshold;
/// then the band's dynamics in lilac. A cut has no threshold or dynamics, only a note saying
/// what it does.
fn band_panel(cx: &mut Context, idx: usize) -> Handle<'_, VStack> {
    VStack::new(cx, move |cx| {
        HStack::new(cx, move |cx| {
            ParamButton::new(cx, Data::params, move |p| &p.threshold.bands[idx].enabled)
                .with_label("")
                .class("panel-on");
            Label::new(cx, &format!("BAND {}", idx + 1)).class("panel-title");

            HStack::new(cx, move |cx| {
                let shapes = [
                    BandShape::LowCut,
                    BandShape::LowShelf,
                    BandShape::Bell,
                    BandShape::HighShelf,
                    BandShape::HighCut,
                ];
                let last = (shapes.len() - 1) as f32;
                for (shape_idx, shape) in shapes.into_iter().enumerate() {
                    SegmentButton::with_content(
                        cx,
                        Data::params,
                        move |p| &p.threshold.bands[idx].shape,
                        shape_idx as f32 / last,
                        move |cx| {
                            ShapeIcon::new(cx, shape);
                        },
                    )
                    .class("shape");
                }
            })
            .class("segmented");

            Element::new(cx).width(Stretch(1.0)).height(Pixels(0.0));

            Button::new(
                cx,
                move |cx| cx.emit(SoloEvent::Toggle(idx)),
                |cx| Label::new(cx, "SOLO"),
            )
            .class("panel-solo")
            .checked(Data::solo_band.map(move |solo_band| *solo_band == Some(idx)));

            Button::new(
                cx,
                |cx| cx.emit(BandSelection::Select(None)),
                |cx| Label::new(cx, "×"),
            )
            .class("panel-close");
        })
        .class("panel-row");

        let is_cut = Data::params.map(move |p| p.threshold.bands[idx].shape.value().is_cut());
        Binding::new(cx, is_cut, move |cx, is_cut| {
            let is_cut = is_cut.get(cx);

            HStack::new(cx, move |cx| {
                labeled_bar(
                    cx,
                    Data::params,
                    move |p| &p.threshold.bands[idx].frequency,
                    "FREQ",
                    "coral",
                );
                labeled_bar(
                    cx,
                    Data::params,
                    move |p| &p.threshold.bands[idx].q,
                    "Q",
                    "coral",
                );
                if !is_cut {
                    labeled_bar(
                        cx,
                        Data::params,
                        move |p| &p.threshold.bands[idx].threshold_db,
                        "THRESHOLD",
                        "coral",
                    );
                }
            })
            .class("bars");

            if is_cut {
                Label::new(
                    cx,
                    "A cut has no threshold: it only sets where the compression stops.",
                )
                .class("panel-note");
            } else {
                // These scale the main ratio and timing, and limit the gain change, in this band
                // only
                VStack::new(cx, move |cx| {
                    HStack::new(cx, |cx| {
                        Label::new(cx, "BAND DYNAMICS").class("trims-title");
                        Element::new(cx).class("trim-rule");
                    })
                    .class("trims-heading");
                    HStack::new(cx, move |cx| {
                        labeled_bar(
                            cx,
                            Data::params,
                            move |p| &p.threshold.bands[idx].sharp,
                            "RATIO x",
                            "lilac",
                        );
                        labeled_bar(
                            cx,
                            Data::params,
                            move |p| &p.threshold.bands[idx].speed,
                            "ATTACK/REL x",
                            "lilac",
                        );
                        labeled_bar(
                            cx,
                            Data::params,
                            move |p| &p.threshold.bands[idx].range_db,
                            "RANGE",
                            "lilac",
                        );
                        VStack::new(cx, move |cx| {
                            Label::new(cx, "MODE").class("bar-name");
                            segmented(
                                cx,
                                Data::params,
                                move |p| &p.threshold.bands[idx].action,
                                &["AUTO", "DOWN", "UP", "OFF"],
                                "lilac",
                            );
                        })
                        .class("bar-cell")
                        .width(Auto);
                    })
                    .class("bars");
                })
                .class("trims");
            }
        });
    })
    .class("band-panel")
}

/// The global controls, one frame per signal colour.
fn banks(cx: &mut Context) {
    HStack::new(cx, |cx| {
        frame(cx, "COMPRESSOR", "coral", |cx| {
            HStack::new(cx, |cx| {
                labeled_bar(cx, Data::params, |p| &p.threshold.threshold_db, "THRESHOLD", "coral");
                labeled_bar(
                    cx,
                    Data::params,
                    |p| &p.compressors.downwards.ratio,
                    "RATIO",
                    "coral",
                );
                labeled_bar(
                    cx,
                    Data::params,
                    |p| &p.compressors.downwards.knee_width_db,
                    "KNEE",
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
                labeled_bar(cx, Data::params, |p| &p.global.output_gain, "OUTPUT", "steel");
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
    .child_bottom(Pixels(20.0));
}
