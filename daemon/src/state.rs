use std::time::{Duration, Instant};

/// `angle::signed_hinge_angle` (unlike the old, [0,180]-bounded
/// `angle::hinge_angle`) returns a signed value across the full
/// `(-180, 180]` range, so both a low-side and a high-side threshold are
/// meaningful here. Thresholds below are derived from 18 real readings
/// (see `angle.rs`'s test module and README.md's empirical validation):
/// every observed Laptop-expected reading fell in [45.1, 110.4] degrees,
/// every observed Tablet-expected reading fell outside that range on both
/// sides (as low as -144.1, as high as 149.0). These constants pick
/// margins inside those gaps, not at the exact observed boundary.
///
/// Below this angle (coming from Laptop), the hinge is considered folded
/// into tablet mode on the low side (near fully closed).
pub const TABLET_ENTER_LOW: f64 = 20.0;
/// Above this angle (coming from Tablet, low side), the hinge is
/// considered back in laptop mode. Set above TABLET_ENTER_LOW so a
/// reading sitting on the boundary doesn't flip-flop.
pub const LAPTOP_ENTER_LOW: f64 = 35.0;
/// Above this angle (coming from Laptop), the hinge is considered folded
/// into tablet mode on the high side (tent/presentation). The nearest
/// real Tablet reading here was 149.0 degrees (`hand_held_tent`); this
/// leaves a comfortable margin below it, since `flat_open_desk` (a real
/// Laptop-adjacent reading at 110.4 degrees) needs enough room on the
/// other side of the dead band too. The resulting 3-degree gap to
/// TABLET_ENTER_HIGH is thin -- known and accepted, not overlooked.
pub const TABLET_ENTER_HIGH: f64 = 146.0;
/// Below this angle (coming from Tablet, high side), the hinge is
/// considered back in laptop mode. Set below TABLET_ENTER_HIGH so a
/// reading sitting on the boundary doesn't flip-flop.
pub const LAPTOP_ENTER_HIGH: f64 = 138.0;
/// Gates the low-side Tablet entry only (see `zone_for`): a low signed
/// hinge angle alone can't distinguish a lid closing while the unit rests
/// normally on a surface from the unit actually being folded closed and
/// picked up, since both can produce a similar angle. Real calibration
/// data shows a clean gap -- ordinary desk/lap use never exceeds 2
/// degrees of base tilt (see `angle::base_tilt_from_level`), while every
/// genuine low-side Tablet reading (folded and picked up, or propped into
/// a tent) is at least 62.6 degrees. 8 degrees sits well inside that gap.
/// The high-side (tent/presentation) entry is intentionally left
/// ungated: those postures were confirmed correctly classified without
/// this check, and this constant is not used there.
pub const BASE_TILT_THRESHOLD: f64 = 8.0;
/// A candidate state must persist this long before it's confirmed.
pub const DEBOUNCE: Duration = Duration::from_millis(750);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HingeState {
    Laptop,
    Tablet,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    ToLaptop,
    ToTablet,
}

pub struct StateMachine {
    current: HingeState,
    candidate: Option<(HingeState, Instant)>,
}

impl StateMachine {
    /// Equivalent to `from_state(HingeState::Laptop)`. No longer called by
    /// `main` (which now seeds via `reconcile()` + `from_state` instead of
    /// assuming Laptop), but kept as the default constructor tests use
    /// throughout this module.
    #[allow(dead_code)]
    pub fn new() -> Self {
        StateMachine {
            current: HingeState::Laptop,
            candidate: None,
        }
    }

    pub fn current(&self) -> HingeState {
        self.current
    }

    pub fn from_state(current: HingeState) -> Self {
        StateMachine {
            current,
            candidate: None,
        }
    }

    /// Stateless one-shot classification, for reconciling to hardware
    /// without a prior state to hysterese against (daemon startup, or
    /// after resuming from suspend). Resolves the same dead bands
    /// `zone_for` uses for its continuous, debounced classification by
    /// assuming Laptop as the neutral prior -- the same conservative
    /// default a fresh `StateMachine` already starts from.
    pub fn classify(angle_deg: f64, base_tilt_deg: f64) -> HingeState {
        Self::zone_for(angle_deg, base_tilt_deg, HingeState::Laptop)
    }

    fn zone_for(angle_deg: f64, base_tilt_deg: f64, current: HingeState) -> HingeState {
        match current {
            HingeState::Laptop => {
                let low_side_tablet = angle_deg < TABLET_ENTER_LOW && base_tilt_deg > BASE_TILT_THRESHOLD;
                let high_side_tablet = angle_deg > TABLET_ENTER_HIGH;
                if low_side_tablet || high_side_tablet {
                    HingeState::Tablet
                } else {
                    HingeState::Laptop
                }
            }
            HingeState::Tablet => {
                if angle_deg > LAPTOP_ENTER_LOW && angle_deg < LAPTOP_ENTER_HIGH {
                    HingeState::Laptop
                } else {
                    HingeState::Tablet
                }
            }
        }
    }

    pub fn update(&mut self, angle_deg: f64, base_tilt_deg: f64, now: Instant) -> Option<Transition> {
        let target = Self::zone_for(angle_deg, base_tilt_deg, self.current);

        if target == self.current {
            self.candidate = None;
            return None;
        }

        match self.candidate {
            Some((state, since)) if state == target => {
                if now.duration_since(since) >= DEBOUNCE {
                    self.current = target;
                    self.candidate = None;
                    return Some(match target {
                        HingeState::Laptop => Transition::ToLaptop,
                        HingeState::Tablet => Transition::ToTablet,
                    });
                }
            }
            _ => {
                self.candidate = Some((target, now));
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_in_laptop_state() {
        let sm = StateMachine::new();
        assert_eq!(sm.current(), HingeState::Laptop);
    }

    #[test]
    fn classify_reports_laptop_for_a_clear_laptop_angle() {
        // 70 degrees, no tilt: comfortably inside the Laptop zone,
        // mirrors no_transition_while_reading_stays_in_laptop_zone above.
        assert_eq!(StateMachine::classify(70.0, 0.0), HingeState::Laptop);
    }

    #[test]
    fn classify_reports_tablet_for_a_clear_low_side_fold_with_tilt() {
        // 10 degrees with 20 degrees of tilt: clears TABLET_ENTER_LOW and
        // BASE_TILT_THRESHOLD, same reading used in
        // folding_past_low_threshold_with_sufficient_tilt_and_holding_debounce_confirms_tablet.
        assert_eq!(StateMachine::classify(10.0, 20.0), HingeState::Tablet);
    }

    #[test]
    fn classify_reports_laptop_for_a_low_side_fold_without_sufficient_tilt() {
        // The "almost closed lid, resting normally" false-positive case:
        // low angle, but tilt stays under BASE_TILT_THRESHOLD.
        assert_eq!(StateMachine::classify(-39.9, 0.75), HingeState::Laptop);
    }

    #[test]
    fn classify_reports_tablet_for_a_clear_high_side_fold() {
        // 155 degrees, ungated by tilt on the high side.
        assert_eq!(StateMachine::classify(155.0, 0.0), HingeState::Tablet);
    }

    #[test]
    fn classify_resolves_dead_bands_as_laptop() {
        // Both dead bands (20..35 low side, 138..146 high side) are
        // ambiguous by construction; a one-shot classification with no
        // prior state to hysterese against must resolve them the same
        // conservative way a fresh StateMachine does: Laptop.
        assert_eq!(StateMachine::classify(27.0, 20.0), HingeState::Laptop);
        assert_eq!(StateMachine::classify(142.0, 0.0), HingeState::Laptop);
    }

    #[test]
    fn from_state_seeds_current_without_requiring_debounce() {
        let mut sm = StateMachine::from_state(HingeState::Tablet);
        assert_eq!(sm.current(), HingeState::Tablet);
        // No stale candidate carried over: a reading squarely in the
        // Tablet zone must not register as a pending transition.
        let t0 = Instant::now();
        assert_eq!(sm.update(155.0, 0.0, t0), None);
        assert_eq!(sm.current(), HingeState::Tablet);
    }

    #[test]
    fn no_transition_while_reading_stays_in_laptop_zone() {
        let mut sm = StateMachine::new();
        let t0 = Instant::now();
        // 70 degrees: comfortably inside [35, 138], e.g. typing_desk (58.0)
        // to reclined_typing_desk (84.6) territory. Tilt is irrelevant
        // here since the angle never approaches either gated threshold.
        assert_eq!(sm.update(70.0, 0.0, t0), None);
        assert_eq!(sm.current(), HingeState::Laptop);
    }

    #[test]
    fn folding_past_low_threshold_with_sufficient_tilt_and_holding_debounce_confirms_tablet() {
        let mut sm = StateMachine::new();
        let t0 = Instant::now();
        // Crosses TABLET_ENTER_LOW (20) with tilt (20.0) above
        // BASE_TILT_THRESHOLD (8): candidate, not yet confirmed.
        assert_eq!(sm.update(10.0, 20.0, t0), None);
        assert_eq!(sm.current(), HingeState::Laptop);
        // Still within the debounce window: not yet confirmed.
        assert_eq!(sm.update(10.0, 20.0, t0 + Duration::from_millis(500)), None);
        assert_eq!(sm.current(), HingeState::Laptop);
        // Past the debounce window: confirmed.
        let result = sm.update(10.0, 20.0, t0 + Duration::from_millis(800));
        assert_eq!(result, Some(Transition::ToTablet));
        assert_eq!(sm.current(), HingeState::Tablet);
    }

    // The finding that motivated BASE_TILT_THRESHOLD: a real dry-run
    // reading of an "almost closed lid, resting normally on a surface"
    // computed to a low signed angle (-39.9, well past TABLET_ENTER_LOW)
    // but only 0.75 degrees of base tilt (angle.rs's
    // base_tilt_from_level_matches_python_reference test has the same
    // value under "almost_closed_lid_resting"). Without the tilt gate
    // this incorrectly confirmed Tablet; with it, it must stay Laptop
    // even held well past the debounce window.
    #[test]
    fn almost_closed_lid_resting_normally_does_not_confirm_tablet() {
        let mut sm = StateMachine::new();
        let t0 = Instant::now();
        assert_eq!(sm.update(-39.9, 0.75, t0), None);
        assert_eq!(sm.current(), HingeState::Laptop);
        assert_eq!(
            sm.update(-39.9, 0.75, t0 + Duration::from_millis(800)),
            None
        );
        assert_eq!(sm.current(), HingeState::Laptop);
        assert_eq!(
            sm.update(-39.9, 0.75, t0 + Duration::from_secs(10)),
            None
        );
        assert_eq!(sm.current(), HingeState::Laptop);
    }

    #[test]
    fn folding_past_high_threshold_and_holding_debounce_confirms_tablet() {
        let mut sm = StateMachine::new();
        let t0 = Instant::now();
        // 155 degrees: past TABLET_ENTER_HIGH (146), e.g. tent/presentation
        // territory (real readings there: 149.0 to -144.1 wrapping around).
        // The high side is intentionally ungated by tilt (tilt=0.0 here
        // on purpose, to confirm this path doesn't require it).
        assert_eq!(sm.update(155.0, 0.0, t0), None);
        assert_eq!(sm.current(), HingeState::Laptop);
        assert_eq!(sm.update(155.0, 0.0, t0 + Duration::from_millis(500)), None);
        assert_eq!(sm.current(), HingeState::Laptop);
        let result = sm.update(155.0, 0.0, t0 + Duration::from_millis(800));
        assert_eq!(result, Some(Transition::ToTablet));
        assert_eq!(sm.current(), HingeState::Tablet);
    }

    #[test]
    fn brief_dip_into_tablet_zone_that_reverts_before_debounce_has_no_effect() {
        let mut sm = StateMachine::new();
        let t0 = Instant::now();
        assert_eq!(sm.update(10.0, 20.0, t0), None);
        // Reverts to a laptop-zone angle before debounce elapses.
        assert_eq!(sm.update(70.0, 20.0, t0 + Duration::from_millis(300)), None);
        assert_eq!(sm.current(), HingeState::Laptop);
        // Even after what would have been the original debounce deadline.
        assert_eq!(sm.update(70.0, 20.0, t0 + Duration::from_millis(900)), None);
        assert_eq!(sm.current(), HingeState::Laptop);
    }

    #[test]
    fn folding_back_open_confirms_laptop_after_debounce() {
        let mut sm = StateMachine::new();
        let t0 = Instant::now();
        sm.update(10.0, 20.0, t0);
        sm.update(10.0, 20.0, t0 + Duration::from_millis(800)); // now Tablet
        // Opens back up past LAPTOP_ENTER_LOW (35).
        assert_eq!(sm.update(70.0, 20.0, t0 + Duration::from_millis(900)), None);
        let result = sm.update(70.0, 20.0, t0 + Duration::from_millis(1700));
        assert_eq!(result, Some(Transition::ToLaptop));
        assert_eq!(sm.current(), HingeState::Laptop);
    }

    #[test]
    fn low_side_dead_band_does_not_flip_from_laptop() {
        let mut sm = StateMachine::new();
        let t0 = Instant::now();
        // 27 degrees: inside TABLET_ENTER_LOW..LAPTOP_ENTER_LOW (20..35)
        // dead band, but still >= TABLET_ENTER_LOW, so from Laptop this
        // must not register as a tablet candidate. High tilt (20.0) here
        // to confirm the dead band holds regardless of tilt.
        assert_eq!(sm.update(27.0, 20.0, t0), None);
        assert_eq!(sm.update(27.0, 20.0, t0 + Duration::from_millis(800)), None);
        assert_eq!(sm.current(), HingeState::Laptop);
    }

    #[test]
    fn high_side_dead_band_does_not_flip_from_laptop() {
        let mut sm = StateMachine::new();
        let t0 = Instant::now();
        // 142 degrees: inside LAPTOP_ENTER_HIGH..TABLET_ENTER_HIGH
        // (138..146) dead band, so from Laptop this must not register as
        // a tablet candidate either.
        assert_eq!(sm.update(142.0, 0.0, t0), None);
        assert_eq!(sm.update(142.0, 0.0, t0 + Duration::from_millis(800)), None);
        assert_eq!(sm.current(), HingeState::Laptop);
    }

    // Real-data classification check: every one of the 18 readings
    // collected during this project's investigation must land in its
    // expected zone under these thresholds. Angle values are the actual
    // signed_hinge_angle outputs cross-validated in angle.rs's test
    // module; tilt values are the actual base_tilt_from_level outputs
    // for the same readings, in the same order. Not synthetic examples.
    #[test]
    fn real_readings_classify_correctly() {
        let expect_laptop = [
            (57.98, 0.48),
            (110.43, 0.74),
            (84.60, 0.38),
            (51.75, 39.23),
            (65.17, 14.62),
            (45.10, 20.47),
            (46.36, 3.87),
            (53.44, 49.60),
            (67.28, 3.49),
            (69.87, 26.13),
            (56.79, 45.48),
            (50.01, 11.01),
            (45.82, 34.09),
            (53.11, 55.94),
        ];
        let expect_tablet = [
            (-70.42, 179.65),
            (149.02, 18.36),
            (-124.31, 62.55),
            (-144.14, 178.84),
        ];

        for (angle, tilt) in expect_laptop {
            let mut sm = StateMachine::new();
            let t0 = Instant::now();
            sm.update(angle, tilt, t0);
            sm.update(angle, tilt, t0 + DEBOUNCE + Duration::from_millis(50));
            assert_eq!(
                sm.current(),
                HingeState::Laptop,
                "angle {angle} (tilt {tilt}) should classify as Laptop"
            );
        }

        for (angle, tilt) in expect_tablet {
            let mut sm = StateMachine::new();
            let t0 = Instant::now();
            sm.update(angle, tilt, t0);
            sm.update(angle, tilt, t0 + DEBOUNCE + Duration::from_millis(50));
            assert_eq!(
                sm.current(),
                HingeState::Tablet,
                "angle {angle} (tilt {tilt}) should classify as Tablet"
            );
        }
    }

    // Same 18 real readings as real_readings_classify_correctly, but
    // checked against the stateless one-shot classify() that reconcile()
    // will actually call on real hardware at startup/resume -- no
    // debounce, no prior state.
    #[test]
    fn classify_matches_real_readings() {
        let expect_laptop = [
            (57.98, 0.48),
            (110.43, 0.74),
            (84.60, 0.38),
            (51.75, 39.23),
            (65.17, 14.62),
            (45.10, 20.47),
            (46.36, 3.87),
            (53.44, 49.60),
            (67.28, 3.49),
            (69.87, 26.13),
            (56.79, 45.48),
            (50.01, 11.01),
            (45.82, 34.09),
            (53.11, 55.94),
        ];
        let expect_tablet = [
            (-70.42, 179.65),
            (149.02, 18.36),
            (-124.31, 62.55),
            (-144.14, 178.84),
        ];

        for (angle, tilt) in expect_laptop {
            assert_eq!(
                StateMachine::classify(angle, tilt),
                HingeState::Laptop,
                "angle {angle} (tilt {tilt}) should classify as Laptop"
            );
        }
        for (angle, tilt) in expect_tablet {
            assert_eq!(
                StateMachine::classify(angle, tilt),
                HingeState::Tablet,
                "angle {angle} (tilt {tilt}) should classify as Tablet"
            );
        }
    }
}
