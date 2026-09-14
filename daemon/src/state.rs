use std::time::{Duration, Instant};

/// `angle::hinge_angle` is bounded to [0, 180] degrees (it's an `arccos`
/// result), so only the "folded near closed" direction (low end of that
/// range) is currently detectable as tablet mode. There is no high-side
/// threshold: a value like 230 or 270 degrees, which would represent
/// tent/presentation mode or a closed lid in a full 360-degree hinge
/// model, can never actually be produced by `hinge_angle`, so no branch
/// here can be written in terms of one. See `angle::hinge_angle`'s doc
/// comment for why, and README.md's known-limitations note.
///
/// Below this angle (coming from Laptop), the hinge is considered folded
/// into tablet mode.
pub const TABLET_ENTER_LOW: f64 = 40.0;
/// Above this angle (coming from Tablet), the hinge is considered back in
/// laptop mode. Set a few degrees above TABLET_ENTER_LOW so a reading
/// sitting on the boundary doesn't flip-flop.
pub const LAPTOP_ENTER_LOW: f64 = 50.0;
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
                if angle_deg < TABLET_ENTER_LOW {
                    HingeState::Tablet
                } else {
                    HingeState::Laptop
                }
            }
            HingeState::Tablet => {
                if angle_deg > LAPTOP_ENTER_LOW {
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
        assert_eq!(sm.update(120.0, t0), None);
        assert_eq!(sm.current(), HingeState::Laptop);
    }

    #[test]
    fn folding_past_threshold_and_holding_debounce_confirms_tablet() {
        let mut sm = StateMachine::new();
        let t0 = Instant::now();
        // Crosses TABLET_ENTER_LOW immediately: candidate, not yet confirmed.
        assert_eq!(sm.update(10.0, t0), None);
        assert_eq!(sm.current(), HingeState::Laptop);
        // Still within the debounce window: not yet confirmed.
        assert_eq!(sm.update(10.0, t0 + Duration::from_millis(500)), None);
        assert_eq!(sm.current(), HingeState::Laptop);
        // Past the debounce window: confirmed.
        let result = sm.update(10.0, t0 + Duration::from_millis(800));
        assert_eq!(result, Some(Transition::ToTablet));
        assert_eq!(sm.current(), HingeState::Tablet);
    }

    #[test]
    fn brief_dip_into_tablet_zone_that_reverts_before_debounce_has_no_effect() {
        let mut sm = StateMachine::new();
        let t0 = Instant::now();
        assert_eq!(sm.update(10.0, t0), None);
        // Reverts to a laptop-zone angle before debounce elapses.
        assert_eq!(sm.update(120.0, t0 + Duration::from_millis(300)), None);
        assert_eq!(sm.current(), HingeState::Laptop);
        // Even after what would have been the original debounce deadline.
        assert_eq!(sm.update(120.0, t0 + Duration::from_millis(900)), None);
        assert_eq!(sm.current(), HingeState::Laptop);
    }

    // A `tent_angle_counts_as_tablet` test previously asserted that 270.0
    // degrees (tent/presentation mode, in a full 360-degree hinge model)
    // registered as tablet mode via a since-removed TABLET_ENTER_HIGH
    // branch. That input is not physically reachable: `angle::hinge_angle`
    // is bounded to [0, 180] (arccos range) and can never return 270.0, so
    // the test was asserting behavior the real system can never exercise.
    // It was removed rather than kept green. Disambiguating tent mode (or
    // a closed lid) from a true tablet fold is a known, currently
    // unimplemented limitation pending new real-hardware measurements; see
    // README.md's known-limitations note.

    #[test]
    fn folding_back_open_confirms_laptop_after_debounce() {
        let mut sm = StateMachine::new();
        let t0 = Instant::now();
        sm.update(10.0, t0);
        sm.update(10.0, t0 + Duration::from_millis(800)); // now Tablet
        // Opens back up past LAPTOP_ENTER_LOW (50).
        assert_eq!(sm.update(120.0, t0 + Duration::from_millis(900)), None);
        let result = sm.update(120.0, t0 + Duration::from_millis(1700));
        assert_eq!(result, Some(Transition::ToLaptop));
        assert_eq!(sm.current(), HingeState::Laptop);
    }

    #[test]
    fn dead_band_between_laptop_and_tablet_enter_thresholds_does_not_flip_from_laptop() {
        let mut sm = StateMachine::new();
        let t0 = Instant::now();
        // 45 degrees: inside TABLET_ENTER_LOW..LAPTOP_ENTER_LOW dead band,
        // but still >= TABLET_ENTER_LOW, so from Laptop this must not
        // register as a tablet candidate.
        assert_eq!(sm.update(45.0, t0), None);
        assert_eq!(
            sm.update(45.0, t0 + Duration::from_millis(800)),
            None
        );
        assert_eq!(sm.current(), HingeState::Laptop);
    }
}
