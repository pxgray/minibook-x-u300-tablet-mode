use std::io;
use std::process::Command;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;

// Same problem daemon/src/display.rs already solves: this binary runs as
// root (see systemd/dsi-blank-retry.service), but Mutter's DisplayConfig
// interface lives on the logged-in user's session bus, which
// authenticates by real uid (SO_PEERCRED) and rejects root outright even
// when pointed at the right socket. runuser -u drops to that user's uid
// before exec'ing busctl, which is what actually lets the session bus
// authenticate the connection -- see daemon/src/display.rs for the same
// pattern.
const TARGET_USER: &str = "pxgray";
const SESSION_BUS_ADDRESS: &str = "unix:path=/run/user/1000/bus";

// DPMS-style encoding used by org.gnome.Mutter.DisplayConfig's
// PowerSaveMode property, confirmed live: 0 = on, 3 = off.
pub const POWER_SAVE_OFF: i32 = 3;
pub const POWER_SAVE_ON: i32 = 0;

/// Builds the busctl args (everything after "busctl" itself) for setting
/// PowerSaveMode to `state`. Verified live for both POWER_SAVE_OFF and
/// POWER_SAVE_ON: exits 0, no error output, and is the exact call
/// confirmed to both reproduce and clear the DSI corruption during
/// testing.
pub fn power_save_toggle_args(state: i32) -> Vec<String> {
    [
        "--address",
        SESSION_BUS_ADDRESS,
        "call",
        "org.gnome.Mutter.DisplayConfig",
        "/org/gnome/Mutter/DisplayConfig",
        "org.freedesktop.DBus.Properties",
        "Set",
        "ssv",
        "org.gnome.Mutter.DisplayConfig",
        "PowerSaveMode",
        "i",
        &state.to_string(),
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

/// Runs the toggle as TARGET_USER via runuser, since this process itself
/// runs as root (see module doc comment above).
pub fn set_power_save_mode(state: i32) -> io::Result<()> {
    let status = Command::new("runuser")
        .args(["-u", TARGET_USER, "--", "busctl"])
        .args(power_save_toggle_args(state))
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "runuser busctl exited with {status}"
        )))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum RepairOutcome {
    /// active was false: detection logged, no toggle was ever attempted.
    DryRun,
    /// No new DSI error appeared within `wait` after this attempt's
    /// toggle; treated as cleared.
    Cleared { attempts: u32 },
    /// All 5 attempts were tried and a new DSI error kept appearing (or
    /// the toggle command kept failing) every time.
    GaveUp { attempts: u32 },
}

const MAX_ATTEMPTS: u32 = 5;

/// The actual state machine, generic over `toggle` so it's testable
/// without shelling out to a real runuser/busctl, and over `wait` so
/// tests don't have to sleep 2 real seconds per attempt. `toggle` should
/// return true if both the off and on calls succeeded.
pub fn attempt_repair_with<F>(rx: &Receiver<()>, toggle: F, wait: Duration) -> RepairOutcome
where
    F: Fn() -> bool,
{
    for attempt in 1..=MAX_ATTEMPTS {
        if !toggle() {
            // Treat a failed command the same as "didn't clear it".
            std::thread::sleep(wait);
            continue;
        }
        match rx.recv_timeout(wait) {
            Ok(()) => continue, // a new error arrived during the window: not cleared yet
            Err(RecvTimeoutError::Timeout) => {
                return RepairOutcome::Cleared { attempts: attempt };
            }
            Err(RecvTimeoutError::Disconnected) => {
                return RepairOutcome::GaveUp { attempts: attempt };
            }
        }
    }
    RepairOutcome::GaveUp {
        attempts: MAX_ATTEMPTS,
    }
}

/// Production entry point: real toggle via set_power_save_mode, real 2s
/// wait. When `active` is false, logs nothing itself (the caller in
/// main.rs owns logging) and returns DryRun immediately without ever
/// calling the toggle.
pub fn attempt_repair(rx: &Receiver<()>, active: bool) -> RepairOutcome {
    if !active {
        return RepairOutcome::DryRun;
    }
    attempt_repair_with(
        rx,
        || set_power_save_mode(POWER_SAVE_OFF).is_ok() && set_power_save_mode(POWER_SAVE_ON).is_ok(),
        Duration::from_secs(2),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_save_toggle_args_off_matches_the_verified_command() {
        assert_eq!(
            power_save_toggle_args(POWER_SAVE_OFF),
            vec![
                "--address",
                "unix:path=/run/user/1000/bus",
                "call",
                "org.gnome.Mutter.DisplayConfig",
                "/org/gnome/Mutter/DisplayConfig",
                "org.freedesktop.DBus.Properties",
                "Set",
                "ssv",
                "org.gnome.Mutter.DisplayConfig",
                "PowerSaveMode",
                "i",
                "3",
            ]
        );
    }

    #[test]
    fn power_save_toggle_args_on_matches_the_verified_command() {
        let args = power_save_toggle_args(POWER_SAVE_ON);
        assert_eq!(args.last().unwrap(), "0");
    }

    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn dry_run_returns_dry_run_and_leaves_channel_untouched() {
        let (tx, rx) = mpsc::channel();
        tx.send(()).unwrap();
        let outcome = attempt_repair(&rx, false);
        assert_eq!(outcome, RepairOutcome::DryRun);
        assert_eq!(rx.try_recv(), Ok(()));
    }

    #[test]
    fn clears_on_the_first_attempt_when_no_new_error_arrives() {
        let (_tx, rx) = mpsc::channel();
        let outcome = attempt_repair_with(&rx, || true, Duration::from_millis(10));
        assert_eq!(outcome, RepairOutcome::Cleared { attempts: 1 });
    }

    #[test]
    fn retries_when_a_new_error_arrives_during_the_window_then_clears() {
        let (tx, rx) = mpsc::channel();
        // Simulate a new DSI error showing up during the first attempt's
        // wait window, but not the second's.
        let call_count = std::sync::atomic::AtomicU32::new(0);
        let toggle = || {
            let n = call_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n == 0 {
                tx.send(()).unwrap();
            }
            true
        };
        let outcome = attempt_repair_with(&rx, toggle, Duration::from_millis(10));
        assert_eq!(outcome, RepairOutcome::Cleared { attempts: 2 });
    }

    #[test]
    fn gives_up_after_five_attempts_if_it_never_clears() {
        let (tx, rx) = mpsc::channel();
        // Keep the channel alive and keep "seeing" a new error every
        // window, forever, by re-sending after every toggle call.
        let toggle = || {
            let _ = tx.send(());
            true
        };
        let outcome = attempt_repair_with(&rx, toggle, Duration::from_millis(10));
        assert_eq!(outcome, RepairOutcome::GaveUp { attempts: 5 });
    }

    #[test]
    fn a_failing_toggle_command_counts_as_a_failed_attempt() {
        let (_tx, rx) = mpsc::channel();
        let call_count = std::sync::atomic::AtomicU32::new(0);
        let toggle = || {
            let n = call_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            // Fails twice, then succeeds with nothing on the channel
            // (i.e. actually clears on the 3rd attempt).
            n >= 2
        };
        let outcome = attempt_repair_with(&rx, toggle, Duration::from_millis(10));
        assert_eq!(outcome, RepairOutcome::Cleared { attempts: 3 });
    }
}
