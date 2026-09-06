//! Abstractions for the parameterized threshold curve.
//!
//! This was previously computed directly inside of the `CompressorBank` but this makes it easier to
//! reuse it when drawing the GUI.

/// The number of local "dents" that can be placed on the threshold curve.
pub const NUM_NODES: usize = 6;

/// A single local dent in the threshold curve. A node is not a filter: it only says where the
/// brake bites harder, by pulling the threshold down around one frequency.
#[derive(Debug, Clone, Copy)]
pub struct NodeSettings {
    pub enabled: bool,
    /// The natural logarithm of the node's center frequency.
    pub ln_freq: f32,
    /// How many decibels the curve is pulled down at the node's center.
    pub depth_db: f32,
    /// The dent's width, as a standard deviation in natural-log-frequency units.
    pub width: f32,
}

impl Default for NodeSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            ln_freq: 0.0,
            depth_db: 0.0,
            width: 0.3,
        }
    }
}

/// Parameters for a curve, similar to the fields found in `ThresholdParams` but using plain floats
/// instead of parameters.
#[derive(Debug, Default, Clone, Copy)]
pub struct CurveParams {
    /// The compressor threshold at the center frequency. When sidechaining is enabled, the input
    /// signal is gained by the inverse of this value. This replaces the input gain in the original
    /// Spectral Compressor. In the polynomial below, this is the intercept.
    pub intercept: f32,
    /// The center frqeuency for the target curve when sidechaining is not enabled. The curve is a
    /// polynomial `threshold_db + curve_slope*x + curve_curve*(x^2)` that evaluates to a decibel
    /// value, where `x = ln(center_frequency) - ln(bin_frequency)`. In other words, this is
    /// evaluated in the log/log domain for decibels and octaves.
    pub center_frequency: f32,
    /// The slope for the curve, in the log/log domain. See the polynomial above.
    pub slope: f32,
    /// The, uh, 'curve' for the curve, in the logarithmic domain. This is the third coefficient in
    /// the quadratic polynomial and controls the parabolic behavior. Positive values turn the curve
    /// into a v-shaped curve, while negative values attenuate everything outside of the center
    /// frequency. See the polynomial above.
    pub curve: f32,
    /// Local dents applied on top of the polynomial. See [`NodeSettings`].
    pub nodes: [NodeSettings; NUM_NODES],
}

/// Evaluates the quadratic threshold curve. This used to be calculated directly inside of the
/// compressor bank since it's so simple, but the editor also needs to compute this so it makes
/// sense to deduplicate it a bit.
///
/// The curve is evaluated in log-log space (so with octaves being the independent variable and gain
/// in decibels being the output of the equation).
pub struct Curve<'a> {
    params: &'a CurveParams,
    /// The natural logarithm of [`CurveParams::cemter_frequency`].
    ln_center_frequency: f32,
}

impl<'a> Curve<'a> {
    pub fn new(params: &'a CurveParams) -> Self {
        Self {
            params,
            ln_center_frequency: params.center_frequency.ln(),
        }
    }

    /// Evaluate the curve for the natural logarithm of the frequency value. This can be used as an
    /// optimization to avoid computing these logarithms all the time.
    #[inline]
    pub fn evaluate_ln(&self, ln_freq: f32) -> f32 {
        let offset = ln_freq - self.ln_center_frequency;
        let mut db = self.params.intercept
            + (self.params.slope * offset)
            + (self.params.curve * offset * offset);

        // Each enabled node pulls the curve down by a gaussian centred on its frequency, so the
        // compressor bites harder there without the rest of the curve moving.
        for node in &self.params.nodes {
            if !node.enabled || node.depth_db == 0.0 {
                continue;
            }

            let distance = ln_freq - node.ln_freq;
            let width = node.width.max(0.01);
            db -= node.depth_db * (-(distance * distance) / (2.0 * width * width)).exp();
        }

        db
    }

    /// Evaluate the curve for a value in Hertz.
    #[inline]
    #[allow(unused)]
    pub fn evaluate_linear(&self, freq: f32) -> f32 {
        self.evaluate_ln(freq.ln())
    }
}
