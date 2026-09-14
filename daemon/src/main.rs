mod accel;
mod acpi;
mod angle;
mod cli;
mod state;
mod uinput;
mod watchdog;

use state::Transition;
use std::process::ExitCode;
use std::time::{Duration, Instant};

const POLL_INTERVAL: Duration = Duration::from_millis(100);
const JERK_THRESHOLD: f64 = 4000.0; // raw units/sec; tune during --dry-run testing.
const WATCHDOG_TIMEOUT: Duration = Duration::from_secs(5);

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
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

    let mut uinput_switch = if opts.dry_run {
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

    let watchdog = watchdog::Watchdog::new();
    if !opts.dry_run {
        let acpi_path = opts.acpi_path.clone();
        watchdog.spawn_monitor(WATCHDOG_TIMEOUT, move || {
            eprintln!("minibookd: watchdog triggered, forcing LTSM(0)");
            if let Err(e) = acpi::set_tablet_mode(&acpi_path, false) {
                eprintln!("minibookd: watchdog revert failed: {e}");
            }
            std::process::exit(1);
        });
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
                continue;
            }
        };
        let base = match accel::read_vector(&opts.base_accel) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("minibookd: failed to read base accelerometer: {e}");
                continue;
            }
        };

        let display_mag = angle::magnitude(display);
        let base_mag = angle::magnitude(base);

        let jerked = match (last_display_mag, last_base_mag) {
            (Some(prev_d), Some(prev_b)) => {
                let jd = angle::jerk(prev_d, display_mag, dt_secs).abs();
                let jb = angle::jerk(prev_b, base_mag, dt_secs).abs();
                jd > JERK_THRESHOLD || jb > JERK_THRESHOLD
            }
            _ => false,
        };
        last_display_mag = Some(display_mag);
        last_base_mag = Some(base_mag);

        let angle_deg = if jerked {
            held_angle.unwrap_or_else(|| angle::hinge_angle(display, base))
        } else {
            let a = angle::hinge_angle(display, base);
            held_angle = Some(a);
            a
        };

        if let Some(transition) = state_machine.update(angle_deg, now) {
            handle_transition(transition, &opts, &mut uinput_switch);
        }

        watchdog.heartbeat();
    }
}

fn handle_transition(
    transition: Transition,
    opts: &cli::Cli,
    uinput_switch: &mut Option<uinput::TabletSwitch>,
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
        if let Some(sw) = uinput_switch {
            if let Err(e) = sw.set(true) {
                eprintln!("minibookd: failed to set uinput switch: {e}");
            }
        }
        if let Err(e) = acpi::set_tablet_mode(&opts.acpi_path, true) {
            eprintln!("minibookd: LTSM(1) failed: {e}");
        }
    } else {
        if let Err(e) = acpi::set_tablet_mode(&opts.acpi_path, false) {
            eprintln!("minibookd: LTSM(0) failed: {e}");
        }
        if let Some(sw) = uinput_switch {
            if let Err(e) = sw.set(false) {
                eprintln!("minibookd: failed to set uinput switch: {e}");
            }
        }
    }
}
