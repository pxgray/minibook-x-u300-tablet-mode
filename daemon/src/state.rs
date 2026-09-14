use std::time::{Duration, Instant};

/// `angle::signed_hinge_angle` (unlike the old, [0,180]-bounded
/// `angle::hinge_angle`) returns a signed value across the full
/// `(-180, 180]` range, so both a low-side and a high-side threshold are
/// meaningful here. Thresholds below are derived from 18 real readings
/// (see `angle.rs`'s test module and README.md's empirical validation):
/// every observed Laptop-expected reading fell in [24.8, 91.6] degrees,
/// every observed Tablet-expected reading fell outside that range on both
/// sides (as low as -162.9, as high as 130.6). These constants pick
/// margins inside those gaps, not at the exact observed boundary.
///
/// Below this angle (coming from Laptop), the hinge is considered folded
/// into tablet mode on the low side (near fully closed).
pub const TABLET_ENTER_LOW: f64 = 0.0;
/// Above this angle (coming from Tablet, low side), the hinge is
/// considered back in laptop mode. Set above TABLET_ENTER_LOW so a
/// reading sitting on the boundary doesn't flip-flop.
pub const LAPTOP_ENTER_LOW: f64 = 15.0;
/// Above this angle (coming from Laptop), the hinge is considered folded
/// into tablet mode on the high side (tent/presentation). The nearest
/// real Tablet reading here was 130.6 degrees (`hand_held_tent`); this
/// leaves a comfortable margin below it, since `flat_open_desk` (a real
/// Laptop-adjacent reading at 91.6 degrees) needs enough room on the
/// other side of the dead band too.
pub const TABLET_ENTER_HIGH: f64 = 128.0;
/// Below this angle (coming from Tablet, high side), the hinge is
/// considered back in laptop mode. Set below TABLET_ENTER_HIGH so a
/// reading sitting on the boundary doesn't flip-flop.
pub const LAPTOP_ENTER_HIGH: f64 = 120.0;
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
    pub fn new() -> Self {
        StateMachine {
            current: HingeState::Laptop,
            candidate: None,
        }
    }

    pub fn current(&self) -> HingeState {
        self.current
    }

    fn zone_for(angle_deg: f64, current: HingeState) -> HingeState {
        match current {
            HingeState::Laptop => {
                if angle_deg < TABLET_ENTER_LOW || angle_deg > TABLET_ENTER_HIGH {
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

    pub fn update(&mut self, angle_deg: f64, now: Instant) -> Option<Transition> {
        let target = Self::zone_for(angle_deg, self.current);

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
    fn no_transition_while_reading_stays_in_laptop_zone() {
        let mut sm = StateMachine::new();
        let t0 = Instant::now();
        // 60 degrees: comfortably inside [15, 120], e.g. typing_desk (37.9)
        // to reclined_typing_desk (65.3) territory.
        assert_eq!(sm.update(60.0, t0), None);
        assert_eq!(sm.current(), HingeState::Laptop);
    }

    #[test]
    fn folding_past_low_threshold_and_holding_debounce_confirms_tablet() {
        let mut sm = StateMachine::new();
        let t0 = Instant::now();
        // Crosses TABLET_ENTER_LOW (0) immediately: candidate, not yet confirmed.
        assert_eq!(sm.update(-10.0, t0), None);
        assert_eq!(sm.current(), HingeState::Laptop);
        // Still within the debounce window: not yet confirmed.
        assert_eq!(sm.update(-10.0, t0 + Duration::from_millis(500)), None);
        assert_eq!(sm.current(), HingeState::Laptop);
        // Past the debounce window: confirmed.
        let result = sm.update(-10.0, t0 + Duration::from_millis(800));
        assert_eq!(result, Some(Transition::ToTablet));
        assert_eq!(sm.current(), HingeState::Tablet);
    }

    #[test]
    fn folding_past_high_threshold_and_holding_debounce_confirms_tablet() {
        let mut sm = StateMachine::new();
        let t0 = Instant::now();
        // 140 degrees: past TABLET_ENTER_HIGH (128), e.g. tent/presentation
        // territory (real readings there: 130.6 to -162.9 wrapping around).
        assert_eq!(sm.update(140.0, t0), None);
        assert_eq!(sm.current(), HingeState::Laptop);
        assert_eq!(sm.update(140.0, t0 + Duration::from_millis(500)), None);
        assert_eq!(sm.current(), HingeState::Laptop);
        let result = sm.update(140.0, t0 + Duration::from_millis(800));
        assert_eq!(result, Some(Transition::ToTablet));
        assert_eq!(sm.current(), HingeState::Tablet);
    }

    #[test]
    fn brief_dip_into_tablet_zone_that_reverts_before_debounce_has_no_effect() {
        let mut sm = StateMachine::new();
        let t0 = Instant::now();
        assert_eq!(sm.update(-10.0, t0), None);
        // Reverts to a laptop-zone angle before debounce elapses.
        assert_eq!(sm.update(60.0, t0 + Duration::from_millis(300)), None);
        assert_eq!(sm.current(), HingeState::Laptop);
        // Even after what would have been the original debounce deadline.
        assert_eq!(sm.update(60.0, t0 + Duration::from_millis(900)), None);
        assert_eq!(sm.current(), HingeState::Laptop);
    }

    #[test]
    fn folding_back_open_confirms_laptop_after_debounce() {
        let mut sm = StateMachine::new();
        let t0 = Instant::now();
        sm.update(-10.0, t0);
        sm.update(-10.0, t0 + Duration::from_millis(800)); // now Tablet
        // Opens back up past LAPTOP_ENTER_LOW (15).
        assert_eq!(sm.update(60.0, t0 + Duration::from_millis(900)), None);
        let result = sm.update(60.0, t0 + Duration::from_millis(1700));
        assert_eq!(result, Some(Transition::ToLaptop));
        assert_eq!(sm.current(), HingeState::Laptop);
    }

    #[test]
    fn low_side_dead_band_does_not_flip_from_laptop() {
        let mut sm = StateMachine::new();
        let t0 = Instant::now();
        // 7 degrees: inside TABLET_ENTER_LOW..LAPTOP_ENTER_LOW (0..15) dead
        // band, but still >= TABLET_ENTER_LOW, so from Laptop this must
        // not register as a tablet candidate.
        assert_eq!(sm.update(7.0, t0), None);
        assert_eq!(sm.update(7.0, t0 + Duration::from_millis(800)), None);
        assert_eq!(sm.current(), HingeState::Laptop);
    }

    #[test]
    fn high_side_dead_band_does_not_flip_from_laptop() {
        let mut sm = StateMachine::new();
        let t0 = Instant::now();
        // 124 degrees: inside LAPTOP_ENTER_HIGH..TABLET_ENTER_HIGH
        // (120..128) dead band, so from Laptop this must not register as
        // a tablet candidate either.
        assert_eq!(sm.update(124.0, t0), None);
        assert_eq!(sm.update(124.0, t0 + Duration::from_millis(800)), None);
        assert_eq!(sm.current(), HingeState::Laptop);
    }

    // Real-data classification check: every one of the 18 readings
    // collected during this project's investigation must land in its
    // expected zone under these thresholds. Values are the actual
    // signed_hinge_angle outputs cross-validated in angle.rs's test
    // module, not synthetic examples.
    #[test]
    fn real_readings_classify_correctly() {
        let expect_laptop = [
            37.9394, 91.6352, 65.2539, 30.8190, 45.7979, 25.3021, 26.1337, 32.4389, 47.5534,
            49.4841, 35.8747, 29.4976, 24.8350, 32.1177,
        ];
        let expect_tablet = [-87.3549, 130.5630, -142.4170, -162.9189];

        for angle in expect_laptop {
            let mut sm = StateMachine::new();
            let t0 = Instant::now();
            sm.update(angle, t0);
            sm.update(angle, t0 + DEBOUNCE + Duration::from_millis(50));
            assert_eq!(
                sm.current(),
                HingeState::Laptop,
                "angle {angle} should classify as Laptop"
            );
        }

        for angle in expect_tablet {
            let mut sm = StateMachine::new();
            let t0 = Instant::now();
            sm.update(angle, t0);
            sm.update(angle, t0 + DEBOUNCE + Duration::from_millis(50));
            assert_eq!(
                sm.current(),
                HingeState::Tablet,
                "angle {angle} should classify as Tablet"
            );
        }
    }
}
