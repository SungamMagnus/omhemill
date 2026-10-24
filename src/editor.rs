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
use crate::curve::{BandShape, NUM_BANDS};
use crate::presets::{self, Preset};
use crate::OmhemillParams;

mod analyzer;
mod controls;

/// The plugin window's size, in logical pixels. This mirrors the proportions of the design.
const GUI_WIDTH: u32 = 1200;
const GUI_HEIGHT: u32 = 645;

/// One colour per band, so you can tell which band a handle, chip or panel belongs to while
/// adjusting it. Spaced around the hue circle at a like lightness, skipping amber, which is kept
/// for live state (the gain reduction).
pub(crate) const BAND_COLORS: [(u8, u8, u8); NUM_BANDS] = [
    (0xed, 0x81, 0x59), // coral
    (0x9b, 0xc4, 0x6a), // moss
    (0x52, 0xb0, 0x8a), // jade
    (0x4f, 0xb3, 0xc4), // teal
    (0x6f, 0x8f, 0xe0), // steel
    (0x9f, 0x7d, 0xe0), // violet
    (0xd3, 0x8f, 0xd9), // orchid
    (0xe0, 0x7a, 0x9a), // rose
];

/// The stylesheet class for a band's colour.
fn band_class(idx: usize) -> String {
    format!("band-{}", idx + 1)
}

/// The per-band colour rules, generated from [`BAND_COLORS`] so the table is the one place the
/// colours live.
fn band_stylesheet() -> String {
    let mut css = String::new();
    for (idx, (r, g, b)) in BAND_COLORS.iter().enumerate() {
        let class = band_class(idx);
        let color = format!("#{r:02x}{g:02x}{b:02x}");
        css.push_str(&format!(
            "
.chip.{class} {{ border-color: {color}59; }}
.chip.{class}.selected {{ border-color: {color}; background-color: {color}24; }}
param-button.chip-dot.{class}:hover {{ border-color: {color}; background-color: {color}24; }}
param-button.chip-dot.{class}:checked {{ border-color: {color}; background-color: {color}; color: #201f1e; }}
.band-panel.{class} {{ border-color: {color}; }}
.panel-title.{class} {{ color: {color}; }}
param-button.panel-on.{class} {{ border-color: {color}; }}
param-button.panel-on.{class}:checked {{ background-color: {color}; }}
segment-button.shape.{class} {{ border-color: {color}73; color: {color}; }}
segment-button.shape.{class}:checked {{ border-color: {color}; background-color: {color}; color: #201f1e; }}
bar-slider.{class} {{ color: {color}; }}
"
        ));
    }
    css
}

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

/// Put a band's shape, Q, threshold, dynamics and mode back to their defaults. It stays on, at
/// its frequency, and its panel stays open.
pub struct ResetBand(pub usize);

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

        event.map(|ResetBand(idx), _| {
            let band = &self.params.threshold.bands[*idx];
            let ptrs = [
                band.shape.as_ptr(),
                band.q.as_ptr(),
                band.threshold_db.as_ptr(),
                band.sharp.as_ptr(),
                band.speed.as_ptr(),
                band.range_db.as_ptr(),
                band.action.as_ptr(),
            ];
            for ptr in ptrs {
                let default = unsafe { ptr.default_normalized_value() };
                cx.emit(RawParamEvent::BeginSetParameter(ptr));
                cx.emit(RawParamEvent::SetParameterNormalized(ptr, default));
                cx.emit(RawParamEvent::EndSetParameter(ptr));
            }
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
    // No built-in theming: vizia's light theme would otherwise paint its own views (dropdowns,
    // popups, buttons, text fields) white under ours. nih-plug's widget layout is still needed.
    create_vizia_editor(editor_state, ViziaTheming::None, move |cx, _| {
        nih_plug_vizia::widgets::register_theme(cx);
        if let Err(err) = cx.add_stylesheet(include_style!("src/editor/theme.css")) {
            nih_error!("Failed to load stylesheet: {err:?}")
        }
        if let Err(err) = cx.add_stylesheet(CSS::from_string(&band_stylesheet())) {
            nih_error!("Failed to load the band colours: {err:?}")
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
            HStack::new(cx, |cx| {
                controls_column(cx);
                bands(cx);
            })
            .class("body");
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
                // Not hoverable, so a click lands on the button itself: vizia only runs a press
                // handler when the press targets that very view
                Label::new(cx, Data::preset_name)
                    .class("preset-current")
                    .hoverable(false);
                Label::new(cx, "\u{25be}").class("preset-arrow").hoverable(false);
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

/// The spectrum display, the selected band's panel in a strip under it, and the row of band
/// chips, in one coral frame. The analyzer draws its own grid, axis labels, spectrum, threshold
/// line and band handles. Nothing sits on top of the spectrum.
fn bands(cx: &mut Context) {
    frame(cx, "BANDS", "coral", |cx| {
        Analyzer::new(cx, Data::analyzer_data, Data::sample_rate, Data::params)
            .width(Stretch(1.0))
            .height(Stretch(1.0));

        band_strip(cx);
        band_chips(cx);
    })
    .width(Stretch(1.0))
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
                    .class("chip-dot")
                    .class(&band_class(idx));

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
                .class("chip-freq")
                .hoverable(false);

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
                .class("chip-depth")
                .hoverable(false);
            })
            .class("chip")
            .class(&band_class(idx))
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

/// The strip under the spectrum: the selected band's panel, or a hint when no band is selected.
fn band_strip(cx: &mut Context) {
    HStack::new(cx, |cx| {
        Binding::new(cx, Data::selected_band, |cx, selected_band| {
            match selected_band.get(cx) {
                Some(idx) => {
                    band_panel(cx, idx);
                }
                None => {
                    Label::new(
                        cx,
                        "Click a band to edit it. Double-click the display to add one.",
                    )
                    .class("band-hint");
                }
            }
        });
    })
    .class("band-strip");
}

/// The band panel, one compact row in the band's colour: on/off, the shape, frequency, Q and
/// threshold, then the band's dynamics in lilac (ratio and timing multipliers, range and mode),
/// then solo and close. A cut has no threshold or dynamics, only a note saying what it does.
fn band_panel(cx: &mut Context, idx: usize) -> Handle<'_, HStack> {
    let color_class = band_class(idx);
    HStack::new(cx, move |cx| {
        let color_class = band_class(idx);

        ParamButton::new(cx, Data::params, move |p| &p.threshold.bands[idx].enabled)
            .with_label("")
            .class("panel-on")
            .class(&color_class);
        Button::new(
            cx,
            move |cx| cx.emit(SoloEvent::Toggle(idx)),
            |cx| Label::new(cx, "SOLO"),
        )
        .class("panel-solo")
        .checked(Data::solo_band.map(move |solo_band| *solo_band == Some(idx)));

        Label::new(cx, &format!("BAND {}", idx + 1))
            .class("panel-title")
            .class(&color_class);

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
                .class("shape")
                .class(&band_class(idx));
            }
        })
        .class("segmented");

        let is_cut = Data::params.map(move |p| p.threshold.bands[idx].shape.value().is_cut());
        Binding::new(cx, is_cut, move |cx, is_cut| {
            let is_cut = is_cut.get(cx);
            let color_class = band_class(idx);

            labeled_bar(
                cx,
                Data::params,
                move |p| &p.threshold.bands[idx].frequency,
                "FREQ",
                &color_class,
            );
            labeled_bar(
                cx,
                Data::params,
                move |p| &p.threshold.bands[idx].q,
                "Q",
                &color_class,
            );

            if is_cut {
                Label::new(cx, "A cut has no threshold: it only sets where compression stops.")
                    .class("panel-note");
                return;
            }

            labeled_bar(
                cx,
                Data::params,
                move |p| &p.threshold.bands[idx].threshold_db,
                "THRESH",
                &color_class,
            );

            // The band's dynamics scale the main ratio and timing, and limit the gain change, in
            // this band only
            Element::new(cx).class("dynamics-rule");
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
                "ATK/REL x",
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
            .class("mode-cell");
        });

        Button::new(
            cx,
            move |cx| cx.emit(ResetBand(idx)),
            |cx| Label::new(cx, "×"),
        )
        .class("panel-close");
    })
    .class("band-panel")
    .class(&color_class)
}

/// The global controls in a column on the left, one frame per signal colour, so the spectrum
/// gets the window's full height.
fn controls_column(cx: &mut Context) {
    VStack::new(cx, |cx| {
        frame(cx, "COMPRESSOR", "coral", |cx| {
            labeled_bar(cx, Data::params, |p| &p.threshold.threshold_db, "THRESHOLD", "coral");
            labeled_bar(cx, Data::params, |p| &p.compressors.downwards.ratio, "RATIO", "coral");
            labeled_bar(
                cx,
                Data::params,
                |p| &p.compressors.downwards.knee_width_db,
                "KNEE",
                "coral",
            );
        })
        .class("column-frame");

        frame(cx, "TIME", "teal", |cx| {
            labeled_bar(cx, Data::params, |p| &p.global.compressor_attack_ms, "ATTACK", "teal");
            labeled_bar(
                cx,
                Data::params,
                |p| &p.global.compressor_release_ms,
                "RELEASE",
                "teal",
            );
        })
        .class("column-frame");

        frame(cx, "OUTPUT", "steel", |cx| {
            labeled_bar(cx, Data::params, |p| &p.global.dry_wet_ratio, "MIX", "steel");
            labeled_bar(cx, Data::params, |p| &p.global.output_gain, "OUTPUT", "steel");
        })
        .class("column-frame");
    })
    .class("controls-column");
}
