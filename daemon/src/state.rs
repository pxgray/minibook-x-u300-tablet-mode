use std::time::{Duration, Instant};

/// Below this angle (coming from Laptop), or above TABLET_ENTER_HIGH,
/// the hinge is considered folded into tablet mode.
pub const TABLET_ENTER_LOW: f64 = 40.0;
pub const TABLET_ENTER_HIGH: f64 = 230.0;
/// Between these angles (coming from Tablet), the hinge is considered
/// back in laptop mode. Set a few degrees inside the TABLET_ENTER_*
/// thresholds so a reading sitting on the boundary doesn't flip-flop.
pub const LAPTOP_ENTER_LOW: f64 = 50.0;
pub const LAPTOP_ENTER_HIGH: f64 = 220.0;
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

    #[test]
    fn tent_angle_counts_as_tablet() {
        let mut sm = StateMachine::new();
        let t0 = Instant::now();
        // 270 degrees: past TABLET_ENTER_HIGH (230), i.e. tent/presentation.
        sm.update(270.0, t0);
        let result = sm.update(270.0, t0 + Duration::from_millis(800));
        assert_eq!(result, Some(Transition::ToTablet));
    }

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
