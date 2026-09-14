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
}
