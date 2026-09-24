mod kmsg;
mod repair;

use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom};
use std::os::unix::fs::OpenOptionsExt;
use std::sync::mpsc;
use std::thread;

// From <asm-generic/fcntl.h>; the crate is std-only, so no libc::O_NONBLOCK.
const O_NONBLOCK: i32 = 0o4000;

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
    // record; seek to the end so the watch loop only sees new records.
    if let Err(e) = file.seek(SeekFrom::End(0)) {
        eprintln!("dsi-blank-retry: failed to seek /dev/kmsg to end: {e}");
        std::process::exit(1);
    }

    // Before this service starts, the boot may already have left the panel
    // corrupted (see kmsg::needs_startup_repair). Read the backlog through a
    // second, non-blocking fd, opened after the seek above so nothing
    // logged in between is missed (at worst it is seen by both).
    let startup_repair = match OpenOptions::new()
        .read(true)
        .custom_flags(O_NONBLOCK)
        .open("/dev/kmsg")
        .and_then(|mut backlog| kmsg::read_backlog(&mut backlog))
    {
        Ok(messages) => kmsg::needs_startup_repair(&messages),
        Err(e) => {
            eprintln!("dsi-blank-retry: could not read /dev/kmsg backlog: {e}");
            false
        }
    };

    let (tx, rx) = mpsc::channel();
    if startup_repair {
        eprintln!("dsi-blank-retry: DSI error from before startup was never cleared");
        let _ = tx.send(());
    }
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
