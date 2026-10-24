//! Abstractions for the parameterized threshold curve and the bands shaping it.
//!
//! This was previously computed directly inside of the `CompressorBank` but this makes it easier to
//! reuse it when drawing the GUI.

use nih_plug::prelude::Enum;
use std::f32::consts::LOG2_E;

/// The number of bands that can shape the threshold curve.
pub const NUM_BANDS: usize = 8;

/// The frequency the pink-noise tilt pivots around. This used to be the `Threshold Center`
/// parameter; its old default is kept so the curve sits where it did.
pub const CURVE_CENTER_HZ: f32 = 420.0;

/// A band ratio below this follows the main Ratio instead of setting its own.
pub const BAND_RATIO_FOLLOWS_MAIN_BELOW: f32 = 1.0;

/// A band's range at or above this is no limit at all.
pub const RANGE_UNLIMITED_DB: f32 = 48.0;
/// How far a band's range limit relaxes towards the edge of the band, so a limit does not end in
/// a hard step where the band's influence fades out.
const RANGE_FADE_DB: f32 = 60.0;

/// The compression mode: downward pulls peaks down, upward lifts what is buried, or both at once.
#[derive(Enum, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    #[default]
    #[id = "suppress"]
    #[name = "Downward"]
    Suppress,
    #[id = "lift"]
    #[name = "Upward"]
    Lift,
    #[id = "both"]
    #[name = "Both"]
    Both,
}

impl Action {
    /// How much of the downwards and the upwards gain change gets through.
    fn weights(self) -> (f32, f32) {
        match self {
            Action::Suppress => (1.0, 0.0),
            Action::Lift => (0.0, 1.0),
            Action::Both => (1.0, 1.0),
        }
    }
}

/// A band's own action. `Auto` follows the global [`Action`].
#[derive(Enum, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum BandAction {
    #[default]
    #[id = "auto"]
    #[name = "Auto"]
    Auto,
    #[id = "suppress"]
    #[name = "Down"]
    Suppress,
    #[id = "lift"]
    #[name = "Up"]
    Lift,
    #[id = "off"]
    #[name = "Off"]
    Off,
}

impl BandAction {
    fn weights(self) -> Option<(f32, f32)> {
        match self {
            BandAction::Auto => None,
            BandAction::Suppress => Some((1.0, 0.0)),
            BandAction::Lift => Some((0.0, 1.0)),
            BandAction::Off => Some((0.0, 0.0)),
        }
    }
}

/// The shape of a band. The cuts do not filter the audio: they fence off where the brake works.
#[derive(Enum, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum BandShape {
    #[id = "low_cut"]
    #[name = "Low Cut"]
    LowCut,
    #[id = "low_shelf"]
    #[name = "Low Shelf"]
    LowShelf,
    #[default]
    #[id = "bell"]
    #[name = "Bell"]
    Bell,
    #[id = "high_shelf"]
    #[name = "High Shelf"]
    HighShelf,
    #[id = "high_cut"]
    #[name = "High Cut"]
    HighCut,
}

impl BandShape {
    pub fn is_cut(self) -> bool {
        matches!(self, BandShape::LowCut | BandShape::HighCut)
    }
}

/// One band, in the plain-float form the curve evaluator wants.
#[derive(Debug, Clone, Copy)]
pub struct BandSettings {
    pub enabled: bool,
    pub shape: BandShape,
    /// The natural logarithm of the band's frequency.
    pub ln_freq: f32,
    /// A bell's width at half height, a shelf's transition or a cut's softness, in octaves.
    pub width_octaves: f32,
    /// How many decibels the threshold moves inside the band. Positive bites harder.
    pub sensitivity_db: f32,
    /// The natural logarithm of the band's ratio multiplier.
    pub ln_ratio: Option<f32>,
    /// The natural logarithm of the band's envelope speed multiplier.
    pub ln_speed: f32,
    /// The most gain change the band allows, in decibels. See [`RANGE_UNLIMITED_DB`].
    pub range_db: f32,
    pub action: BandAction,
}

impl Default for BandSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            shape: BandShape::Bell,
            ln_freq: 1_000.0f32.ln(),
            width_octaves: 1.0,
            sensitivity_db: 0.0,
            ln_ratio: None,
            ln_speed: 0.0,
            range_db: RANGE_UNLIMITED_DB,
            action: BandAction::Auto,
        }
    }
}

impl BandSettings {
    /// How much this band covers a frequency, from 0 to 1, given that frequency's distance to the
    /// band in octaves. Cuts cover nothing: they only mask.
    #[inline]
    fn membership(&self, octaves: f32) -> f32 {
        let width = self.width_octaves.max(0.01);
        match self.shape {
            // A gaussian whose full width at half maximum is the band's width
            BandShape::Bell => {
                let sigma = width / 2.355;
                (-0.5 * (octaves / sigma) * (octaves / sigma)).exp()
            }
            BandShape::LowShelf => sigmoid(-octaves * 4.0 / width),
            BandShape::HighShelf => sigmoid(octaves * 4.0 / width),
            BandShape::LowCut | BandShape::HighCut => 0.0,
        }
    }

    /// How much of a frequency is heard while this band is soloed, from 0 to 1: the range a
    /// bell or shelf covers, or the range a cut lets the brake work in.
    #[inline]
    pub fn audition(&self, octaves: f32) -> f32 {
        if self.shape.is_cut() {
            self.mask(octaves)
        } else {
            self.membership(octaves)
        }
    }

    /// How much of the brake a cut lets through at a frequency, from 0 to 1.
    #[inline]
    fn mask(&self, octaves: f32) -> f32 {
        let width = self.width_octaves.max(0.01);
        match self.shape {
            BandShape::LowCut => sigmoid(octaves * 4.0 / width),
            BandShape::HighCut => sigmoid(-octaves * 4.0 / width),
            _ => 1.0,
        }
    }
}

/// A band's Q from its width in octaves, the way an analog EQ's bandwidth relates to its Q.
pub fn q_from_octaves(octaves: f32) -> f32 {
    let ratio = 2.0f32.powf(octaves);
    ratio.sqrt() / (ratio - 1.0)
}

/// The inverse of [`q_from_octaves()`].
pub fn octaves_from_q(q: f32) -> f32 {
    2.0 * (1.0 / (2.0 * q)).asinh() / std::f32::consts::LN_2
}

#[inline]
fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

/// Parameters for a curve, similar to the fields found in `ThresholdParams` but using plain floats
/// instead of parameters.
#[derive(Debug, Default, Clone, Copy)]
pub struct CurveParams {
    /// The compressor threshold at [`CURVE_CENTER_HZ`]. When sidechaining is enabled, the input
    /// signal is gained by the inverse of this value. This replaces the input gain in the original
    /// Spectral Compressor.
    pub intercept: f32,
    /// The slope for the curve in decibels per octave, around [`CURVE_CENTER_HZ`]. This is the
    /// pink noise tilt in the internal mode and flat when sidechaining.
    pub slope: f32,
    pub action: Action,
    pub bands: [BandSettings; NUM_BANDS],
}

/// Everything the bands say about a single frequency.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BinShape {
    /// How far the threshold moves here. Positive means the brake bites harder.
    pub sensitivity_db: f32,
    /// What the global ratio is multiplied by here.
    /// How much bands with their own ratio cover this frequency, from 0 (the main Ratio applies)
    /// to 1 (theirs does).
    pub ratio_weight: f32,
    /// The natural logarithm of the ratio those bands ask for, averaged by how much each covers
    /// this frequency.
    pub ln_ratio: f32,
    /// What the envelope followers' speed is multiplied by here.
    pub speed: f32,
    /// The most gain change allowed here, or infinity.
    pub range_db: f32,
    /// How much of the downwards gain change gets through, cuts included.
    pub down_weight: f32,
    /// How much of the upwards gain change gets through, cuts included.
    pub up_weight: f32,
}

impl BinShape {
    /// The ratio at this frequency, given the main one: the bands' own ratio as far as they reach,
    /// the main one elsewhere, blended in the log domain.
    pub fn ratio(&self, main_ratio: f32) -> f32 {
        ((1.0 - self.ratio_weight) * main_ratio.ln() + self.ratio_weight * self.ln_ratio).exp()
    }
}

/// Evaluates the threshold curve. The curve is evaluated in log-log space (so with octaves being
/// the independent variable and gain in decibels being the output of the equation).
pub struct Curve<'a> {
    params: &'a CurveParams,
    ln_center_frequency: f32,
}

impl<'a> Curve<'a> {
    pub fn new(params: &'a CurveParams) -> Self {
        Self {
            params,
            ln_center_frequency: CURVE_CENTER_HZ.ln(),
        }
    }

    /// The curve before the bands, for the natural logarithm of a frequency.
    #[inline]
    pub fn base_ln(&self, ln_freq: f32) -> f32 {
        let octaves = (ln_freq - self.ln_center_frequency) * LOG2_E;
        self.params.intercept + (self.params.slope * octaves)
    }

    /// The bands' summed sensitivity at the natural logarithm of a frequency.
    #[inline]
    pub fn sensitivity_ln(&self, ln_freq: f32) -> f32 {
        self.params
            .bands
            .iter()
            .filter(|band| band.enabled && !band.shape.is_cut() && band.sensitivity_db != 0.0)
            .map(|band| band.sensitivity_db * band.membership((ln_freq - band.ln_freq) * LOG2_E))
            .sum()
    }

    /// The downwards threshold for the natural logarithm of the frequency. Sensitivity pulls it
    /// down, so the brake bites harder there.
    #[inline]
    pub fn evaluate_ln(&self, ln_freq: f32) -> f32 {
        self.base_ln(ln_freq) - self.sensitivity_ln(ln_freq)
    }

    /// Evaluate the downwards threshold for a value in Hertz.
    #[inline]
    #[allow(unused)]
    pub fn evaluate_linear(&self, freq: f32) -> f32 {
        self.evaluate_ln(freq.ln())
    }

    /// Everything the bands say about the natural logarithm of a frequency.
    pub fn shape_ln(&self, ln_freq: f32) -> BinShape {
        let (mut down_weight, mut up_weight) = self.params.action.weights();
        let mut sensitivity_db = 0.0;
        let mut mask = 1.0;
        let mut ratio_weight = 0.0;
        let mut ln_ratio = 0.0;
        let mut ln_speed = 0.0;
        let mut range_db = f32::INFINITY;

        for band in self.params.bands.iter().filter(|band| band.enabled) {
            let octaves = (ln_freq - band.ln_freq) * LOG2_E;
            if band.shape.is_cut() {
                mask *= band.mask(octaves);
                continue;
            }

            let membership = band.membership(octaves);
            if membership < 1e-4 {
                continue;
            }

            sensitivity_db += band.sensitivity_db * membership;
            // A band's own ratio takes over from the main one as far as the band reaches
            if let Some(band_ln_ratio) = band.ln_ratio {
                ratio_weight += membership;
                ln_ratio += band_ln_ratio * membership;
            }
            // The speed multipliers blend in the log domain, so x2 and x0.5 cancel out
            ln_speed += band.ln_speed * membership;
            if band.range_db < RANGE_UNLIMITED_DB {
                range_db = range_db.min(band.range_db + (1.0 - membership) * RANGE_FADE_DB);
            }
            if let Some((down, up)) = band.action.weights() {
                down_weight += (down - down_weight) * membership;
                up_weight += (up - up_weight) * membership;
            }
        }

        BinShape {
            sensitivity_db,
            ratio_weight: ratio_weight.min(1.0),
            ln_ratio: if ratio_weight > 0.0 {
                ln_ratio / ratio_weight
            } else {
                0.0
            },
            speed: ln_speed.exp(),
            range_db,
            down_weight: down_weight * mask,
            up_weight: up_weight * mask,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bell(freq: f32, width_octaves: f32, sensitivity_db: f32) -> BandSettings {
        BandSettings {
            enabled: true,
            ln_freq: freq.ln(),
            width_octaves,
            sensitivity_db,
            ..BandSettings::default()
        }
    }

    fn params_with(bands: &[BandSettings]) -> CurveParams {
        let mut params = CurveParams {
            intercept: -12.0,
            slope: -3.0,
            ..CurveParams::default()
        };
        params.bands[..bands.len()].copy_from_slice(bands);
        params
    }

    #[test]
    fn q_and_octaves_convert_both_ways() {
        // One octave is the textbook Q of 1.41
        assert!((q_from_octaves(1.0) - 2.0f32.sqrt()).abs() < 1e-4);
        for octaves in [0.05f32, 0.25, 1.0, 2.5] {
            assert!((octaves_from_q(q_from_octaves(octaves)) - octaves).abs() < 1e-4);
        }
    }

    #[test]
    fn neutral_bands_change_nothing() {
        let params = params_with(&[bell(1_000.0, 1.0, 0.0), bell(5_000.0, 0.3, 0.0)]);
        let curve = Curve::new(&params);
        let shape = curve.shape_ln(1_000.0f32.ln());

        assert_eq!(shape.sensitivity_db, 0.0);
        assert_eq!(shape.ratio_weight, 0.0);
        assert_eq!(shape.speed, 1.0);
        assert_eq!(shape.range_db, f32::INFINITY);
        assert_eq!((shape.down_weight, shape.up_weight), (1.0, 0.0));
        assert_eq!(curve.evaluate_linear(CURVE_CENTER_HZ), -12.0);
    }

    #[test]
    fn bell_is_full_at_its_center_and_half_at_its_width() {
        let params = params_with(&[bell(2_000.0, 1.0, 9.0)]);
        let curve = Curve::new(&params);

        assert!((curve.sensitivity_ln(2_000.0f32.ln()) - 9.0).abs() < 1e-4);
        // Half an octave out is the half-height point of a one-octave-wide bell
        let half_octave_up = (2_000.0 * 2.0f32.sqrt()).ln();
        assert!((curve.sensitivity_ln(half_octave_up) - 4.5).abs() < 0.05);
        // Positive sensitivity lowers the threshold, so the brake bites harder there
        assert!(curve.evaluate_linear(2_000.0) < curve.base_ln(2_000.0f32.ln()));
    }

    #[test]
    fn shelves_reach_full_sensitivity_on_their_side() {
        let params = params_with(&[BandSettings {
            shape: BandShape::HighShelf,
            ..bell(4_000.0, 1.0, 6.0)
        }]);
        let curve = Curve::new(&params);

        assert!(curve.sensitivity_ln(16_000.0f32.ln()) > 5.9);
        assert!(curve.sensitivity_ln(1_000.0f32.ln()) < 0.1);
    }

    #[test]
    fn cuts_fence_off_the_brake() {
        let params = params_with(&[
            BandSettings {
                shape: BandShape::LowCut,
                ..bell(100.0, 0.25, 0.0)
            },
            BandSettings {
                shape: BandShape::HighCut,
                ..bell(10_000.0, 0.25, 0.0)
            },
        ]);
        let curve = Curve::new(&params);

        assert!(curve.shape_ln(40.0f32.ln()).down_weight < 0.01);
        assert!(curve.shape_ln(1_000.0f32.ln()).down_weight > 0.99);
        assert!(curve.shape_ln(18_000.0f32.ln()).down_weight < 0.01);
    }

    #[test]
    fn trims_and_actions_apply_inside_the_band_only() {
        let params = params_with(&[BandSettings {
            ln_ratio: Some(4.0f32.ln()),
            ln_speed: 4.0f32.ln(),
            range_db: 6.0,
            action: BandAction::Lift,
            ..bell(3_000.0, 0.5, 0.0)
        }]);
        let curve = Curve::new(&params);

        let inside = curve.shape_ln(3_000.0f32.ln());
        assert!((inside.ratio(2.0) - 4.0).abs() < 1e-3);
        assert!((inside.speed - 4.0).abs() < 1e-3);
        assert!((inside.range_db - 6.0).abs() < 1e-4);
        assert!(inside.down_weight < 1e-4 && (inside.up_weight - 1.0).abs() < 1e-4);

        let outside = curve.shape_ln(300.0f32.ln());
        assert_eq!(outside.ratio(2.0), 2.0);
        assert_eq!(outside.range_db, f32::INFINITY);
        assert_eq!((outside.down_weight, outside.up_weight), (1.0, 0.0));
    }
}
