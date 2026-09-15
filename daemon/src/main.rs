mod accel;
mod acpi;
mod angle;
mod cli;
mod display;
mod state;
mod uinput;
mod watchdog;

use signal_hook::consts::SIGUSR1;
use signal_hook::iterator::Signals;
use state::{HingeState, Transition};
use std::io;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const POLL_INTERVAL: Duration = Duration::from_millis(100);
// Fraction of magnitude per second; tune during real --dry-run testing.
// Dimensionless so it applies evenly across sensors with different raw
// counts-per-g (see the jerk computation in the poll loop below).
const JERK_THRESHOLD_FRACTION: f64 = 1.5;
const WATCHDOG_TIMEOUT: Duration = Duration::from_secs(5);

const USAGE: &str = "minibookd: hinge-angle tablet-mode daemon for the Chuwi MiniBook X

Usage: minibookd [OPTIONS]

Options:
  --dry-run                 Log what would happen; never touch real ACPI/uinput
  --revert-only             Call LTSM(0) once and exit (used by ExecStopPost)
  --acpi-path <path>        Override the LTSM ACPI method path
  --display-accel <path>    Override the display accelerometer IIO device path
  --base-accel <path>       Override the base accelerometer IIO device path
  --help, -h                Print this message and exit";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }

    let opts = match cli::parse(&args) {
        Ok(opts) => Arc::new(opts),
        Err(e) => {
            eprintln!("minibookd: {e}");
            return ExitCode::FAILURE;
        }
    };

    if opts.revert_only {
        return match acpi::set_tablet_mode(&opts.acpi_path, false) {
            Ok(_) => {
                println!("minibookd: revert complete");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("minibookd: revert failed: {e}");
                ExitCode::FAILURE
            }
        };
    }

    let uinput_switch = if opts.dry_run {
        None
    } else {
        match uinput::TabletSwitch::new() {
            Ok(s) => Some(s),
            Err(e) => {
                eprintln!("minibookd: failed to create uinput switch: {e}");
                return ExitCode::FAILURE;
            }
        }
    };
    // Shared (not just owned by the poll loop) so the watchdog and signal
    // handler below can also clear SW_TABLET_MODE before the process exits.
    // Without this, GNOME only ever sees the device vanish while still
    // reporting Tablet mode, never an explicit "leaving tablet mode" event,
    // and (empirically, see README) can leave its own compositor-level
    // input handling and orientation state stuck as a result -- not just
    // the EC-level keyboard/touchpad disable that acpi::set_tablet_mode
    // addresses.
    let uinput_switch = Arc::new(Mutex::new(uinput_switch));

    // Startup reconciliation: a prior unclean exit (a crash between LTSM(1)
    // and reverting, or a manual test script whose revert didn't fire), or
    // simply never having run before, can leave the EC's keyboard-disable
    // register and GNOME's last-known switch state out of sync with the
    // unit's actual physical orientation. reconcile() re-reads the current
    // hinge angle and forces hardware to match it, rather than assuming
    // Laptop outright -- a unit that happens to boot already folded closed
    // should come up in Tablet, not have its keyboard force-enabled while
    // folded shut.
    let initial_state = reconcile(&opts, &uinput_switch);
    let state_machine = Arc::new(Mutex::new(state::StateMachine::from_state(initial_state)));

    let watchdog = watchdog::Watchdog::new();
    if !opts.dry_run {
        let acpi_path = opts.acpi_path.clone();
        let sw = Arc::clone(&uinput_switch);
        watchdog.spawn_monitor(WATCHDOG_TIMEOUT, move || {
            eprintln!("minibookd: watchdog triggered, forcing LTSM(0)");
            if let Err(e) = acpi::set_tablet_mode(&acpi_path, false) {
                eprintln!("minibookd: watchdog revert failed: {e}");
            }
            if let Some(sw) = lock_switch(&sw).as_mut() {
                if let Err(e) = sw.set(false) {
                    eprintln!("minibookd: watchdog uinput revert failed: {e}");
                }
            }
            std::process::exit(1);
        });

        // Without this, SIGINT/SIGTERM (Ctrl+C, systemd stop) use the
        // default disposition and kill the process immediately, skipping
        // the revert below entirely -- if a transition to Tablet had
        // fired, the keyboard/touchpad would stay disabled with no
        // in-process recovery, same failure mode the startup
        // reconciliation above already exists to clean up after the fact.
        // Also clears the uinput switch (see its comment above) so GNOME
        // sees an explicit "leaving tablet mode" event rather than just
        // the device disappearing mid-Tablet.
        let acpi_path = opts.acpi_path.clone();
        let sw = Arc::clone(&uinput_switch);
        ctrlc::set_handler(move || {
            eprintln!("minibookd: caught termination signal, reverting to Laptop state");
            let acpi_result = acpi::set_tablet_mode(&acpi_path, false);
            if let Err(e) = &acpi_result {
                eprintln!("minibookd: signal revert failed: {e}");
            }
            if let Some(sw) = lock_switch(&sw).as_mut() {
                if let Err(e) = sw.set(false) {
                    eprintln!("minibookd: signal uinput revert failed: {e}");
                }
            }
            std::process::exit(if acpi_result.is_ok() { 0 } else { 1 });
        })
        .expect("failed to install signal handler");

        // No suspend/resume awareness exists otherwise: the poll loop and
        // watchdog both use Instant, which does not advance across
        // suspend, so nothing here would ever notice a resume on its own.
        // A systemd-sleep drop-in script (see systemd/system-sleep/) sends
        // SIGUSR1 on post-resume; re-running reconcile() re-reads the
        // actual hinge angle rather than trusting whatever the state
        // machine last believed, in case the EC reset its keyboard-disable
        // register across suspend independently of the daemon.
        let mut signals =
            Signals::new([SIGUSR1]).expect("failed to install SIGUSR1 handler");
        let opts_for_signal = Arc::clone(&opts);
        let sw = Arc::clone(&uinput_switch);
        let sm = Arc::clone(&state_machine);
        thread::spawn(move || {
            for _ in signals.forever() {
                eprintln!("minibookd: caught SIGUSR1 (resume), reconciling to current hinge angle");
                let target = reconcile(&opts_for_signal, &sw);
                *lock_state(&sm) = state::StateMachine::from_state(target);
            }
        });
    }

    let mut last_display_mag: Option<f64> = None;
    let mut last_base_mag: Option<f64> = None;
    let mut held_angle: Option<f64> = None;
    let mut last_tick = Instant::now();

    loop {
        std::thread::sleep(POLL_INTERVAL);
        let now = Instant::now();
        let dt_secs = now.duration_since(last_tick).as_secs_f64();
        last_tick = now;

        let display = match accel::read_vector(&opts.display_accel) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("minibookd: failed to read display accelerometer: {e}");
                // Intentionally skip watchdog.heartbeat() below: persistent
                // sensor read failures should eventually be treated like a
                // hang and trigger the watchdog's revert, not go unnoticed
                // forever.
                continue;
            }
        };
        let base = match accel::read_vector(&opts.base_accel) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("minibookd: failed to read base accelerometer: {e}");
                // See the comment on the display-accelerometer read above:
                // skipping the heartbeat here is intentional.
                continue;
            }
        };

        let display_mag = angle::magnitude(display);
        let base_mag = angle::magnitude(base);

        // Jerk is normalized to a fraction of magnitude per second, not
        // compared as a raw units/sec threshold: the display and base
        // accelerometers have roughly 2x different raw counts-per-g
        // (~809 vs. ~1620 per README.md's measurements), so a shared raw
        // threshold would be twice as sensitive on one sensor as the
        // other. Dividing by the previous magnitude puts both sensors on
        // the same physical scale regardless of their raw counts-per-g.
        let jerked = match (last_display_mag, last_base_mag) {
            (Some(prev_d), Some(prev_b)) => {
                let jd = (angle::jerk(prev_d, display_mag, dt_secs) / prev_d).abs();
                let jb = (angle::jerk(prev_b, base_mag, dt_secs) / prev_b).abs();
                jd > JERK_THRESHOLD_FRACTION || jb > JERK_THRESHOLD_FRACTION
            }
            _ => false,
        };
        last_display_mag = Some(display_mag);
        last_base_mag = Some(base_mag);

        // signed_hinge_angle takes (base, display), the reverse of
        // hinge_angle's (display, base) -- and returns Option<f64> since a
        // degenerate near-parallel-to-hinge-axis orientation is possible
        // (not expected in normal use).
        let angle_deg = if jerked {
            held_angle.unwrap_or_else(|| angle::signed_hinge_angle(base, display).unwrap_or(0.0))
        } else {
            match angle::signed_hinge_angle(base, display) {
                Some(a) => {
                    held_angle = Some(a);
                    a
                }
                None => {
                    eprintln!("minibookd: signed_hinge_angle degenerate this tick, skipping");
                    continue;
                }
            }
        };

        // Gates the state machine's low-side Tablet entry: distinguishes a
        // lid closing while the unit rests normally on a surface from the
        // unit actually being folded closed and picked up. See
        // state::BASE_TILT_THRESHOLD and angle::base_tilt_from_level.
        let base_tilt_deg = angle::base_tilt_from_level(base);

        let transition = lock_state(&state_machine).update(angle_deg, base_tilt_deg, now);
        if let Some(transition) = transition {
            handle_transition(transition, &opts, &uinput_switch);
        }

        if opts.dry_run {
            println!(
                "minibookd: [dry-run] angle={angle_deg:.1} tilt={base_tilt_deg:.1} state={:?}",
                lock_state(&state_machine).current()
            );
        }

        watchdog.heartbeat();
    }
}

fn lock_switch(
    switch: &Mutex<Option<uinput::TabletSwitch>>,
) -> std::sync::MutexGuard<'_, Option<uinput::TabletSwitch>> {
    switch.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn lock_state(state_machine: &Mutex<state::StateMachine>) -> std::sync::MutexGuard<'_, state::StateMachine> {
    state_machine
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn handle_transition(
    transition: Transition,
    opts: &cli::Cli,
    uinput_switch: &Mutex<Option<uinput::TabletSwitch>>,
) {
    let target = match transition {
        Transition::ToTablet => HingeState::Tablet,
        Transition::ToLaptop => HingeState::Laptop,
    };

    if opts.dry_run {
        println!(
            "minibookd: [dry-run] would transition to {}",
            if target == HingeState::Tablet { "Tablet" } else { "Laptop" }
        );
        return;
    }

    if let Err(e) = apply_state(target, opts, uinput_switch) {
        if target == HingeState::Laptop {
            eprintln!(
                "minibookd: LTSM(0) failed, exiting so systemd's ExecStopPost can retry the revert: {e}"
            );
            std::process::exit(1);
        }
    }
}

/// Writes `target` to hardware (LTSM + uinput switch), unconditionally --
/// callers decide whether dry-run should skip calling this at all, and
/// whether a failure is fatal. Shared by `handle_transition` (the debounced
/// live-poll path) and `reconcile` (the one-shot startup/resume path) so
/// the actual ACPI/uinput/display-reset calls exist in exactly one place.
fn apply_state(
    target: HingeState,
    opts: &cli::Cli,
    uinput_switch: &Mutex<Option<uinput::TabletSwitch>>,
) -> io::Result<()> {
    if target == HingeState::Tablet {
        if let Some(sw) = lock_switch(uinput_switch).as_mut() {
            if let Err(e) = sw.set(true) {
                eprintln!("minibookd: failed to set uinput switch: {e}");
            }
        }
        let result = acpi::set_tablet_mode(&opts.acpi_path, true);
        if let Err(e) = &result {
            eprintln!("minibookd: LTSM(1) failed: {e}");
        }
        result.map(|_| ())
    } else {
        let result = acpi::set_tablet_mode(&opts.acpi_path, false);
        if let Some(sw) = lock_switch(uinput_switch).as_mut() {
            if let Err(e) = sw.set(false) {
                eprintln!("minibookd: failed to set uinput switch: {e}");
            }
        }
        // Best-effort: Mutter was empirically observed leaving an explicit
        // 90-degree rotation in place after this transition instead of
        // resetting to the panel_orientation-corrected landscape default
        // (see README's Empirical validation section). Not
        // safety-critical like the LTSM(0) revert above, so a failure here
        // is logged but doesn't change the outcome.
        if let Err(e) = display::reset_rotation() {
            eprintln!("minibookd: failed to reset display rotation: {e}");
        }
        result.map(|_| ())
    }
}

/// Re-reads both accelerometers and classifies the unit's actual current
/// orientation, defaulting to Laptop on any read failure or a degenerate
/// angle -- the same fail-safe direction the watchdog and `--revert-only`
/// already use.
fn classify_current_orientation(opts: &cli::Cli) -> HingeState {
    let display = accel::read_vector(&opts.display_accel);
    let base = accel::read_vector(&opts.base_accel);
    match (display, base) {
        (Ok(display), Ok(base)) => match angle::signed_hinge_angle(base, display) {
            Some(angle_deg) => {
                let tilt_deg = angle::base_tilt_from_level(base);
                state::StateMachine::classify(angle_deg, tilt_deg)
            }
            None => {
                eprintln!("minibookd: reconcile: degenerate hinge angle, defaulting to Laptop");
                HingeState::Laptop
            }
        },
        (display, base) => {
            if let Err(e) = &display {
                eprintln!("minibookd: reconcile: failed to read display accelerometer: {e}");
            }
            if let Err(e) = &base {
                eprintln!("minibookd: reconcile: failed to read base accelerometer: {e}");
            }
            eprintln!("minibookd: reconcile: defaulting to Laptop");
            HingeState::Laptop
        }
    }
}

/// Reconciles hardware to the unit's actual current orientation. Used both
/// at process startup (replacing the old hardcoded "always force Laptop")
/// and on resume from suspend (see the SIGUSR1 handler in `main`), since a
/// unit can be physically folded into Tablet shape in either case. Returns
/// the classified state so the caller can (re)seed the running
/// `StateMachine` to match what was just written to hardware.
///
/// Best-effort like the old startup reconciliation, not fatal like
/// `handle_transition`'s live ToLaptop path: a transient ACPI failure here
/// shouldn't crash-loop the whole daemon before its main loop ever starts,
/// or kill it mid-run over a resume-time hiccup.
fn reconcile(opts: &cli::Cli, uinput_switch: &Mutex<Option<uinput::TabletSwitch>>) -> HingeState {
    let target = classify_current_orientation(opts);

    if opts.dry_run {
        println!("minibookd: [dry-run] reconcile: would apply {target:?}");
    } else if let Err(e) = apply_state(target, opts, uinput_switch) {
        eprintln!("minibookd: reconcile: failed to apply {target:?}: {e}");
    }

    target
}
