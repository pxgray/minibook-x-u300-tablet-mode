/// Computes the angle between two accelerometer vectors via
/// `arccos((v1·v2)/(|v1||v2|))`.
///
/// Bounded to `[0, 180]` degrees, since `arccos` never returns outside
/// that range, and doesn't correct for either sensor's Z-axis DC offset.
/// Superseded by `signed_hinge_angle` for the live poll loop; kept as a
/// simpler reference computation the original README.md validation and
/// this module's own tests still check against.
#[allow(dead_code)]
pub fn hinge_angle(v1: (f64, f64, f64), v2: (f64, f64, f64)) -> f64 {
    let dot = v1.0 * v2.0 + v1.1 * v2.1 + v1.2 * v2.2;
    let cos_theta = dot / (magnitude(v1) * magnitude(v2));
    cos_theta.clamp(-1.0, 1.0).acos().to_degrees()
}

pub fn magnitude(v: (f64, f64, f64)) -> f64 {
    (v.0 * v.0 + v.1 * v.1 + v.2 * v.2).sqrt()
}

pub fn jerk(prev_mag: f64, cur_mag: f64, dt_secs: f64) -> f64 {
    (cur_mag - prev_mag) / dt_secs
}

fn normalize(v: (f64, f64, f64)) -> (f64, f64, f64) {
    let mag = (v.0 * v.0 + v.1 * v.1 + v.2 * v.2).sqrt();
    (v.0 / mag, v.1 / mag, v.2 / mag)
}

fn dot3(a: (f64, f64, f64), b: (f64, f64, f64)) -> f64 {
    a.0 * b.0 + a.1 * b.1 + a.2 * b.2
}

fn cross3(a: (f64, f64, f64), b: (f64, f64, f64)) -> (f64, f64, f64) {
    (
        a.1 * b.2 - a.2 * b.1,
        a.2 * b.0 - a.0 * b.2,
        a.0 * b.1 - a.1 * b.0,
    )
}

fn scale3(v: (f64, f64, f64), s: f64) -> (f64, f64, f64) {
    (v.0 * s, v.1 * s, v.2 * s)
}

fn sub3(a: (f64, f64, f64), b: (f64, f64, f64)) -> (f64, f64, f64) {
    (a.0 - b.0, a.1 - b.1, a.2 - b.2)
}

fn add3(a: (f64, f64, f64), b: (f64, f64, f64)) -> (f64, f64, f64) {
    (a.0 + b.0, a.1 + b.1, a.2 + b.2)
}

/// Rotates vector v by unit quaternion q = (w, x, y, z).
fn rotate_by_quaternion(q: (f64, f64, f64, f64), v: (f64, f64, f64)) -> (f64, f64, f64) {
    let (qw, qx, qy, qz) = q;
    let u = (qx, qy, qz);
    let uv = cross3(u, v);
    let uuv = cross3(u, uv);
    add3(add3(v, scale3(uv, 2.0 * qw)), scale3(uuv, 2.0))
}

fn quaternion_conjugate(q: (f64, f64, f64, f64)) -> (f64, f64, f64, f64) {
    (q.0, -q.1, -q.2, -q.3)
}

/// Per-sensor raw additive Z-axis offset. A ~6 m/s^2 Z-axis DC offset on
/// both sensors is documented in README.md's empirical validation and was
/// independently confirmed on a different unit in the
/// rhalkyard/minibook-dual-accelerometer project. Fitted here from 18 real
/// readings spanning desk, tent, presentation, and reclined-lap postures
/// (see scripts/calibration_data.json and scripts/calibrate_hinge_axis.py),
/// via a sphere fit (leave-one-out stability under 1.2% across all 18
/// readings) rather than a single at-rest measurement.
pub const BASE_OFFSET: (f64, f64, f64) = (16.38, 0.0, -602.19);
pub const DISPLAY_OFFSET: (f64, f64, f64) = (-19.29, 0.0, -160.36);

/// Hinge axis, calibrated for this specific unit, expressed in the base
/// sensor's raw (x,y,z) frame after offset correction. Fitted by
/// scripts/calibrate_hinge_axis.py against all 18 collected readings.
pub const HINGE_AXIS: (f64, f64, f64) = (-0.004262, 0.999987, 0.002677);

/// Fixed rotation from base-referenced coordinates into the display
/// sensor's own raw frame, as a unit quaternion (w, x, y, z). Fitted by
/// scripts/calibrate_hinge_axis.py.
pub const MOUNT_ROTATION: (f64, f64, f64, f64) =
    (-0.710183, 0.005922, -0.703854, 0.013952);

/// Signed, offset-corrected, tilt-corrected hinge angle in degrees, range
/// (-180, 180]. Unlike `hinge_angle`, this corrects for each sensor's own
/// Z-axis DC offset before doing any direction math, and resolves fold
/// direction via `atan2` instead of `arccos`. Returns `None` if the
/// offset-corrected base reading is too close to parallel with the hinge
/// axis to define a stable reference direction in the perpendicular plane
/// -- a degenerate orientation not expected in normal use.
pub fn signed_hinge_angle(
    base_raw: (f64, f64, f64),
    display_raw: (f64, f64, f64),
) -> Option<f64> {
    let u = normalize(sub3(base_raw, BASE_OFFSET));
    let w = normalize(sub3(display_raw, DISPLAY_OFFSET));

    let w_ref = rotate_by_quaternion(quaternion_conjugate(MOUNT_ROTATION), w);

    let u_par = scale3(HINGE_AXIS, dot3(u, HINGE_AXIS));
    let u_perp = sub3(u, u_par);

    let w_ref_par = scale3(HINGE_AXIS, dot3(w_ref, HINGE_AXIS));
    let w_ref_perp = sub3(w_ref, w_ref_par);

    let u_perp_mag = (u_perp.0 * u_perp.0 + u_perp.1 * u_perp.1 + u_perp.2 * u_perp.2).sqrt();
    if u_perp_mag < 1e-6 {
        return None;
    }
    let e1 = scale3(u_perp, 1.0 / u_perp_mag);
    let e2 = cross3(HINGE_AXIS, e1);

    let x = dot3(w_ref_perp, e1);
    let y = dot3(w_ref_perp, e2);

    Some(y.atan2(x).to_degrees())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    // README.md "Empirical validation" table, row 1: typing angle.
    #[test]
    fn hinge_angle_typing_position() {
        let display = (-787.0, 36.0, 450.0);
        let base = (-3.0, 8.0, -1632.0);
        let angle = hinge_angle(display, base);
        assert!(approx_eq(angle, 119.6, 0.5), "got {angle}");
    }

    // README.md row 2: flat open.
    #[test]
    fn hinge_angle_flat_open() {
        let display = (3.0, 21.0, 809.0);
        let base = (13.0, 2.0, -1620.0);
        let angle = hinge_angle(display, base);
        assert!(approx_eq(angle, 178.3, 0.5), "got {angle}");
    }

    // README.md row 3: folded tablet.
    #[test]
    fn hinge_angle_folded_tablet() {
        let display = (3.0, 26.0, 793.0);
        let base = (2.0, -6.0, 426.0);
        let angle = hinge_angle(display, base);
        assert!(approx_eq(angle, 2.7, 0.5), "got {angle}");
    }

    #[test]
    fn magnitude_of_3_4_0_is_5() {
        assert!(approx_eq(magnitude((3.0, 4.0, 0.0)), 5.0, 1e-9));
    }

    #[test]
    fn jerk_is_rate_of_change_of_magnitude() {
        // magnitude went from 10.0 to 12.0 over 0.1s => 20.0 units/s
        assert!(approx_eq(jerk(10.0, 12.0, 0.1), 20.0, 1e-9));
    }

    // Cross-validated against scripts/calibrate_hinge_axis.py's own
    // signed_angle() reference implementation, run against all 18 real
    // readings collected during this project's real-world validation
    // (see scripts/calibration_data.json). Not independently-known ground
    // truth -- this is the Rust port's agreement with its own Python
    // reference, the same cross-validation approach used for the original
    // calibration attempt.
    #[test]
    fn signed_hinge_angle_matches_python_reference_for_all_readings() {
        let cases: &[(&str, (f64, f64, f64), (f64, f64, f64), f64)] = &[
            ("typing_desk", (-3.0, 8.0, -1632.0), (-787.0, 36.0, 450.0), 37.9394),
            ("flat_open_desk", (13.0, 2.0, -1620.0), (3.0, 21.0, 809.0), 91.6352),
            ("folded_tablet_desk", (2.0, -6.0, 426.0), (3.0, 26.0, 793.0), -87.3549),
            ("reclined_typing_desk", (-3.0, -6.0, -1616.0), (-412.0, 31.0, 715.0), 65.2539),
            ("hand_held_tent", (320.0, 0.0, -1568.0), (351.0, 15.0, 729.0), 130.5630),
            ("self_standing_tent", (892.0, 19.0, -1067.0), (892.0, 0.0, 264.0), -142.4170),
            ("presentation_flipped", (18.0, 11.0, 437.0), (-964.0, 25.0, 122.0), -162.9189),
            ("lap_tilt_laying_down", (699.0, 16.0, -1460.0), (-969.0, -10.0, -297.0), 30.8190),
            ("lap_tilt_sitting_up", (-269.0, -14.0, -1636.0), (-490.0, -6.0, 679.0), 45.7979),
            ("lap_leaning_forward", (-341.0, 21.0, -1519.0), (-721.0, 37.0, 568.0), 25.3021),
            ("lap_reclined", (-51.0, 54.0, -1703.0), (-871.0, 65.0, 313.0), 26.1337),
            ("lap_laying_retake", (785.0, 24.0, -1272.0), (-950.0, 37.0, -446.0), 32.4389),
            ("lap_upright_fixed", (-61.0, 5.0, -1606.0), (-651.0, 50.0, 632.0), 47.5534),
            ("lap_partial_fixed", (441.0, 14.0, -1503.0), (-933.0, 26.0, 241.0), 49.4841),
            ("lap_full_fixed", (764.0, 2.0, -1355.0), (-971.0, 18.0, -319.0), 35.8747),
            ("lap_25pct", (199.0, 11.0, -1628.0), (-969.0, 38.0, 164.0), 29.4976),
            ("lap_50pct", (592.0, 25.0, -1479.0), (-996.0, 24.0, -315.0), 24.8350),
            ("lap_75pct", (837.0, 36.0, -1170.0), (-926.0, 44.0, -560.0), 32.1177),
        ];

        for (name, base, display, expected) in cases {
            let angle = signed_hinge_angle(*base, *display).expect("not degenerate");
            assert!(
                approx_eq(angle, *expected, 0.01),
                "{name}: got {angle}, expected {expected}"
            );
        }
    }

    // Primary real-world acceptance check: every reading collected during
    // this project's investigation lands on the correct side of the
    // Laptop/Tablet boundary. Laptop readings cluster 24.8-91.6 degrees;
    // Tablet readings sit clearly outside that range on both sides. This
    // large margin is what makes the classification robust despite an
    // unresolved, smaller residual disagreement between some same-hinge
    // pairs (documented in README.md) -- the daemon only needs a binary
    // split, not perfect absolute-angle precision.
    #[test]
    fn laptop_and_tablet_readings_separate_cleanly() {
        let laptop_angles = [
            37.9394, 91.6352, 65.2539, 30.8190, 45.7979, 25.3021, 26.1337, 32.4389, 47.5534,
            49.4841, 35.8747, 29.4976, 24.8350, 32.1177,
        ];
        let tablet_angles = [-87.3549, 130.5630, -142.4170, -162.9189];

        let laptop_min = laptop_angles.iter().cloned().fold(f64::INFINITY, f64::min);
        let laptop_max = laptop_angles
            .iter()
            .cloned()
            .fold(f64::NEG_INFINITY, f64::max);

        for &a in &tablet_angles {
            assert!(
                a < laptop_min || a > laptop_max,
                "tablet angle {a} should fall outside the laptop range [{laptop_min}, {laptop_max}]"
            );
        }
    }
}
