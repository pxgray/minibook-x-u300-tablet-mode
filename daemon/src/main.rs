mod accel;
mod acpi;
mod angle;
mod cli;
mod display;
mod state;
mod uinput;
mod watchdog;

use state::Transition;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
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
        Ok(opts) => opts,
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

    // Startup reconciliation: the daemon always begins its own bookkeeping
    // in Laptop state and only acts on a *transition*, but a prior unclean
    // exit (a crash between LTSM(1) and reverting, or a manual test script
    // whose revert didn't fire) can leave the EC's keyboard-disable
    // register set and GNOME's last-known switch state stale even though
    // this fresh process's state machine has never observed a transition.
    // Unconditionally reconcile to known-good Laptop state here, once, up
    // front, regardless of what the state machine believes. Best-effort:
    // log failures but don't treat them as fatal, unlike the safety-critical
    // ToLaptop transition handler below.
    if opts.dry_run {
        println!("minibookd: [dry-run] would perform startup reconciliation to Laptop state (LTSM(0), SW_TABLET_MODE=0)");
    } else {
        if let Err(e) = acpi::set_tablet_mode(&opts.acpi_path, false) {
            eprintln!("minibookd: startup reconciliation LTSM(0) failed: {e}");
        }
        if let Some(sw) = lock_switch(&uinput_switch).as_mut() {
            if let Err(e) = sw.set(false) {
                eprintln!("minibookd: startup reconciliation uinput set failed: {e}");
            }
        }
    }

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
    }

    let mut state_machine = state::StateMachine::new();
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

        if let Some(transition) = state_machine.update(angle_deg, base_tilt_deg, now) {
            handle_transition(transition, &opts, &uinput_switch);
        }

        if opts.dry_run {
            println!(
                "minibookd: [dry-run] angle={angle_deg:.1} tilt={base_tilt_deg:.1} state={:?}",
                state_machine.current()
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

fn handle_transition(
    transition: Transition,
    opts: &cli::Cli,
    uinput_switch: &Mutex<Option<uinput::TabletSwitch>>,
) {
    let entering_tablet = matches!(transition, Transition::ToTablet);

    if opts.dry_run {
        println!(
            "minibookd: [dry-run] would transition to {}",
            if entering_tablet { "Tablet" } else { "Laptop" }
        );
        return;
    }

    if entering_tablet {
        if let Some(sw) = lock_switch(uinput_switch).as_mut() {
            if let Err(e) = sw.set(true) {
                eprintln!("minibookd: failed to set uinput switch: {e}");
            }
        }
        if let Err(e) = acpi::set_tablet_mode(&opts.acpi_path, true) {
            eprintln!("minibookd: LTSM(1) failed: {e}");
        }
    } else {
        if let Err(e) = acpi::set_tablet_mode(&opts.acpi_path, false) {
            eprintln!(
                "minibookd: LTSM(0) failed, exiting so systemd's ExecStopPost can retry the revert: {e}"
            );
            std::process::exit(1);
        }
        if let Some(sw) = lock_switch(uinput_switch).as_mut() {
            if let Err(e) = sw.set(false) {
                eprintln!("minibookd: failed to set uinput switch: {e}");
            }
        }
        // Best-effort: Mutter was empirically observed leaving an explicit
        // 90-degree rotation in place after this transition instead of
        // resetting to the panel_orientation-corrected landscape default
        // (see README's Empirical validation section). Not
        // safety-critical like the reverts above, so a failure here is
        // logged but doesn't change the transition's outcome.
        if let Err(e) = display::reset_rotation() {
            eprintln!("minibookd: failed to reset display rotation: {e}");
        }
    }
}
