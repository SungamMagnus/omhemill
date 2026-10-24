//! Factory and user presets.
//!
//! A preset is a name and a set of normalized parameter values keyed by parameter ID. Anything a
//! preset does not mention is set back to its default when the preset loads, so a preset always
//! describes the whole sound. Delta and bypass are monitoring switches and are never touched.
//!
//! Factory presets are built in code from plain values, so they stay right if a parameter's range
//! changes. User presets are JSON files in the user's preset folder.

use nih_plug::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::curve::{Action, BandAction, BandShape};
use crate::OmhemillParams;

/// Parameters a preset never stores or changes.
const MONITORING_PARAM_IDS: [&str; 2] = ["delta", "bypass"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    pub name: String,
    /// Normalized values by parameter ID.
    pub params: BTreeMap<String, f32>,
    /// Factory presets cannot be overwritten.
    #[serde(skip)]
    pub factory: bool,
}

/// The factory presets followed by the user's own, sorted by name.
pub fn all(params: &OmhemillParams) -> Vec<Preset> {
    let mut presets = factory(params);
    presets.extend(load_user());
    presets
}

/// The current state as a preset with the given name.
pub fn capture(params: &OmhemillParams, name: &str) -> Preset {
    let values = params
        .param_map()
        .into_iter()
        .filter(|(id, _, _)| !MONITORING_PARAM_IDS.contains(&id.as_str()))
        .map(|(id, ptr, _)| (id, unsafe { ptr.unmodulated_normalized_value() }))
        .collect();

    Preset {
        name: name.to_owned(),
        params: values,
        factory: false,
    }
}

/// Every parameter a preset sets when it loads, with the normalized value it sets: the preset's
/// value where it has one, otherwise the default.
pub fn values_to_apply(params: &OmhemillParams, preset: &Preset) -> Vec<(ParamPtr, f32)> {
    params
        .param_map()
        .into_iter()
        .filter(|(id, _, _)| !MONITORING_PARAM_IDS.contains(&id.as_str()))
        .map(|(id, ptr, _)| {
            let value = preset
                .params
                .get(&id)
                .copied()
                .unwrap_or_else(|| unsafe { ptr.default_normalized_value() });
            (ptr, value.clamp(0.0, 1.0))
        })
        .collect()
}

/// Where user presets live.
pub fn user_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join("Library/Application Support/Sungam/Omhemill"));
    #[cfg(target_os = "windows")]
    let base = std::env::var_os("APPDATA").map(|appdata| PathBuf::from(appdata).join("Sungam/Omhemill"));
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .map(|data| data.join("sungam/omhemill"));

    base.map(|base| base.join("Presets"))
}

/// The user's presets, sorted by name. Files that cannot be read are skipped.
pub fn load_user() -> Vec<Preset> {
    let Some(dir) = user_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut presets: Vec<Preset> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "json"))
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .filter_map(|json| serde_json::from_str::<Preset>(&json).ok())
        .collect();
    presets.sort_by_key(|preset| preset.name.to_lowercase());

    presets
}

/// Write a user preset, replacing any user preset with the same name.
pub fn save_user(preset: &Preset) -> std::io::Result<PathBuf> {
    let dir = user_dir().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "no preset folder on this system")
    })?;
    std::fs::create_dir_all(&dir)?;

    let path = dir.join(format!("{}.json", file_stem(&preset.name)));
    let json = serde_json::to_string_pretty(preset)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    std::fs::write(&path, json)?;

    Ok(path)
}

/// A file name for a preset name: anything a file system might object to becomes `-`.
fn file_stem(name: &str) -> String {
    let stem: String = name
        .trim()
        .chars()
        .map(|c| if c.is_alphanumeric() || " -_()".contains(c) { c } else { '-' })
        .collect();
    if stem.is_empty() {
        String::from("Untitled")
    } else {
        stem
    }
}

/// Builds a factory preset from plain values, looking the parameter IDs up as it goes.
struct FactoryPreset<'a> {
    map: Vec<(String, ParamPtr, String)>,
    preset: Preset,
    params: &'a OmhemillParams,
}

impl<'a> FactoryPreset<'a> {
    fn new(params: &'a OmhemillParams, name: &str) -> Self {
        Self {
            map: params.param_map(),
            preset: Preset {
                name: name.to_owned(),
                params: BTreeMap::new(),
                factory: true,
            },
            params,
        }
    }

    fn set<P: Param>(mut self, param: &P, plain: P::Plain) -> Self {
        let ptr = param.as_ptr();
        let id = self
            .map
            .iter()
            .find(|(_, candidate, _)| *candidate == ptr)
            .map(|(id, _, _)| id.clone())
            .expect("every parameter is in the parameter map");
        self.preset.params.insert(id, param.preview_normalized(plain));
        self
    }

    /// Switch a band on with a shape, frequency, Q, and threshold relative to the main one.
    fn band(self, idx: usize, shape: BandShape, freq: f32, q: f32, threshold_db: f32) -> Self {
        let band = &self.params.threshold.bands[idx];
        self.set(&band.enabled, true)
            .set(&band.shape, shape)
            .set(&band.frequency, freq)
            .set(&band.q, q)
            .set(&band.threshold_db, threshold_db)
    }

    fn done(self) -> Preset {
        self.preset
    }
}

/// The presets that ship with the plugin.
fn factory(params: &OmhemillParams) -> Vec<Preset> {
    let global = &params.global;
    let threshold = &params.threshold;
    let sharpness = &params.compressors.downwards.ratio;
    let selectivity = &params.compressors.downwards.knee_width_db;
    let bands = &threshold.bands;

    vec![
        FactoryPreset::new(params, "Init").done(),
        FactoryPreset::new(params, "Gentle Resonance Control")
            .set(&threshold.threshold_db, -20.0)
            .set(sharpness, 3.0)
            .set(selectivity, 6.0)
            .set(&global.compressor_attack_ms, 15.0)
            .set(&global.compressor_release_ms, 120.0)
            .band(0, BandShape::LowCut, 40.0, 5.8, 0.0)
            .done(),
        FactoryPreset::new(params, "Vocal De-harsh")
            .set(&threshold.threshold_db, -24.0)
            .set(sharpness, 4.0)
            .set(&global.compressor_attack_ms, 5.0)
            .set(&global.compressor_release_ms, 80.0)
            .band(0, BandShape::LowCut, 120.0, 5.8, 0.0)
            .band(1, BandShape::Bell, 300.0, 1.2, 6.0)
            .band(2, BandShape::Bell, 3_000.0, 1.8, -6.0)
            .band(3, BandShape::Bell, 7_000.0, 2.9, -8.0)
            .set(&bands[3].speed, 2.0)
            .done(),
        FactoryPreset::new(params, "De-ess")
            .set(&threshold.threshold_db, -28.0)
            .set(sharpness, 6.0)
            .set(&global.compressor_attack_ms, 1.0)
            .set(&global.compressor_release_ms, 40.0)
            .band(0, BandShape::LowCut, 3_500.0, 2.9, 0.0)
            .band(1, BandShape::Bell, 7_000.0, 1.4, -6.0)
            .set(&bands[1].speed, 2.0)
            .done(),
        FactoryPreset::new(params, "Mix Bus Smooth")
            .set(&threshold.threshold_db, -30.0)
            .set(sharpness, 2.0)
            .set(selectivity, 12.0)
            .set(&global.compressor_attack_ms, 30.0)
            .set(&global.compressor_release_ms, 250.0)
            .set(&global.dry_wet_ratio, 0.6)
            .band(0, BandShape::LowCut, 60.0, 5.8, 0.0)
            .done(),
        FactoryPreset::new(params, "Lift Detail")
            .set(&threshold.action, Action::Lift)
            .set(&threshold.threshold_db, -40.0)
            .set(sharpness, 2.0)
            .set(&global.compressor_attack_ms, 50.0)
            .set(&global.compressor_release_ms, 300.0)
            .band(0, BandShape::LowShelf, 200.0, 1.4, 6.0)
            .set(&bands[0].action, BandAction::Off)
            .band(1, BandShape::HighShelf, 6_000.0, 1.4, -4.0)
            .set(&bands[1].range_db, 6.0)
            .done(),
        FactoryPreset::new(params, "Spectral OTT")
            .set(&threshold.action, Action::Both)
            .set(&threshold.threshold_db, -30.0)
            .set(sharpness, 4.0)
            .set(&global.compressor_attack_ms, 10.0)
            .set(&global.compressor_release_ms, 100.0)
            .set(&global.dry_wet_ratio, 0.5)
            .done(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> OmhemillParams {
        let (analyzer_input, _analyzer_output) = triple_buffer::TripleBuffer::default().split();
        let bank = crate::compressor_bank::CompressorBank::new(analyzer_input, 2, 4096);
        OmhemillParams::new(&bank)
    }

    #[test]
    fn factory_presets_build_and_stay_in_range() {
        let params = params();
        let presets = factory(&params);

        assert_eq!(presets[0].name, "Init");
        assert!(presets[0].params.is_empty());
        for preset in &presets {
            assert!(preset.factory);
            assert!(preset.params.values().all(|value| (0.0..=1.0).contains(value)));
        }
    }

    #[test]
    fn init_applies_every_default_and_skips_monitoring() {
        let params = params();
        let init = &factory(&params)[0];
        let applied = values_to_apply(&params, init);

        assert_eq!(applied.len(), params.param_map().len() - MONITORING_PARAM_IDS.len());
        for (ptr, value) in applied {
            assert_eq!(value, unsafe { ptr.default_normalized_value() });
        }
    }

    #[test]
    fn capture_round_trips_through_json() {
        let params = params();
        let preset = capture(&params, "Mine");
        let json = serde_json::to_string(&preset).unwrap();
        let back: Preset = serde_json::from_str(&json).unwrap();

        assert_eq!(back.name, "Mine");
        assert_eq!(back.params, preset.params);
        assert!(!back.params.contains_key("delta") && !back.params.contains_key("bypass"));
    }

    #[test]
    fn file_names_are_safe() {
        assert_eq!(file_stem("Vocal / bright?"), "Vocal - bright-");
        assert_eq!(file_stem("  "), "Untitled");
    }
}
