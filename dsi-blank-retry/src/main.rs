mod kmsg;
mod repair;

use std::fs::File;
use std::io::{Seek, SeekFrom};
use std::sync::mpsc;
use std::thread;

fn main() {
    let active = std::env::var("DSI_BLANK_RETRY_ACTIVE")
        .map(|v| v == "1")
        .unwrap_or(false);
    eprintln!("dsi-blank-retry: starting (active={active})");

    let mut file = File::open("/dev/kmsg").unwrap_or_else(|e| {
        eprintln!("dsi-blank-retry: failed to open /dev/kmsg: {e}");
        std::process::exit(1);
    });

    // A fresh /dev/kmsg reader replays the whole ring buffer from the oldest
    // record; seek to the end so historical DSI errors are not acted on.
    if let Err(e) = file.seek(SeekFrom::End(0)) {
        eprintln!("dsi-blank-retry: failed to seek /dev/kmsg to end: {e}");
        std::process::exit(1);
    }

    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut source = file;
        if let Err(e) = kmsg::watch(&mut source, &tx) {
            eprintln!("dsi-blank-retry: /dev/kmsg watch loop ended: {e}");
        }
        std::process::exit(1);
    });

    loop {
        match rx.recv() {
            Ok(()) => {
                eprintln!("dsi-blank-retry: DSI error detected");
                match repair::attempt_repair(&rx, active) {
                    repair::RepairOutcome::DryRun => {
                        eprintln!("dsi-blank-retry: dry-run (active=0), not repairing");
                    }
                    repair::RepairOutcome::Cleared { attempts } => {
                        eprintln!("dsi-blank-retry: cleared after {attempts} attempt(s)");
                    }
                    repair::RepairOutcome::GaveUp { attempts } => {
                        eprintln!(
                            "dsi-blank-retry: gave up after {attempts} attempts, corruption may still be present"
                        );
                    }
                }
            }
            Err(_) => {
                eprintln!("dsi-blank-retry: watch thread's channel closed, exiting");
                std::process::exit(1);
            }
        }
    }
}
