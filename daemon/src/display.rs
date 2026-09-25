use std::io;
use std::process::Command;

// This unit's only display and its known-good logical-monitor config
// (connector, mode, position, scale), confirmed via `busctl --user call
// ... GetCurrentState` while correctly showing landscape. Hardcoded per
// this repo's single-unit convention (see CLAUDE.md) rather than read
// back live each time: if the user changes display scale in GNOME
// Settings, this will silently reset it to 1.25 on the next
// Tablet->Laptop transition. Acceptable tradeoff for now.
const CONNECTOR: &str = "DSI-1";
const MODE_ID: &str = "1920x1200@50.000";
const SCALE: &str = "1.25";

// The daemon runs as root (see systemd/minibookd.service) but Mutter's
// DisplayConfig interface lives on the logged-in user's session bus, not
// root's own -- this unit has a single user account, uid 1000. Pointing
// --address at that socket is not enough on its own: the session bus
// authenticates connections by the connecting process's real uid (SO_PEERCRED),
// and empirically rejects root outright ("Transport endpoint is not
// connected"), so the busctl call itself must actually run as that user,
// not just as root talking to that user's socket -- see reset_rotation,
// which wraps it in `runuser -u`.
const SESSION_BUS_ADDRESS: &str = "unix:path=/run/user/1000/bus";
const TARGET_USER: &str = "pxgray";

const APPLY_MONITORS_CONFIG_SIGNATURE: &str = "uua(iiduba(ssa{sv}))a{sv}";

pub fn reset_rotation_args() -> Vec<String> {
    [
        "--address",
        SESSION_BUS_ADDRESS,
        "call",
        "org.gnome.Mutter.DisplayConfig",
        "/org/gnome/Mutter/DisplayConfig",
        "org.gnome.Mutter.DisplayConfig",
        "ApplyMonitorsConfig",
        APPLY_MONITORS_CONFIG_SIGNATURE,
        "1", // serial (GetCurrentState's serial is not checked by Mutter
        // for ApplyMonitorsConfig in practice, but the argument is required)
        "1", // method: 1 = temporary, doesn't rewrite monitors.xml, so a
        // manual change made between two of our transitions is only
        // overridden until the next one rather than permanently clobbered
        "1",
        "0",
        "0",
        SCALE,
        "0",
        "true", // logical_monitors[0]: x, y, scale, transform=normal, primary
        "1",
        CONNECTOR,
        MODE_ID,
        "0", // monitors[0]: connector, mode_id, {}
        "0", // top-level properties: {}
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

/// Forces Mutter's monitor rotation back to normal (transform=0). Mutter
/// was empirically observed leaving an explicit 90-degree transform in
/// place after the Tablet->Laptop transition, overriding the kernel's
/// panel_orientation correction and showing portrait instead of the
/// correct landscape -- see README's Empirical validation section. This
/// is a workaround for that Mutter behavior, not a fix for our own code,
/// and is best-effort: a failure here doesn't affect keyboard/touchpad
/// safety, so callers should log and continue rather than treat it as fatal.
pub fn reset_rotation() -> io::Result<()> {
    // runuser drops to TARGET_USER's real uid before exec'ing busctl, which
    // is what actually lets the session bus authenticate the connection --
    // running busctl directly as root and merely pointing --address at the
    // user's socket path was tried and empirically fails, see
    // SESSION_BUS_ADDRESS's comment.
    let status = Command::new("runuser")
        .args(["-u", TARGET_USER, "--", "busctl"])
        .args(reset_rotation_args())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "runuser busctl exited with {status}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_expected_busctl_args() {
        assert_eq!(
            reset_rotation_args(),
            vec![
                "--address",
                "unix:path=/run/user/1000/bus",
                "call",
                "org.gnome.Mutter.DisplayConfig",
                "/org/gnome/Mutter/DisplayConfig",
                "org.gnome.Mutter.DisplayConfig",
                "ApplyMonitorsConfig",
                "uua(iiduba(ssa{sv}))a{sv}",
                "1",
                "1",
                "1",
                "0",
                "0",
                "1.25",
                "0",
                "true",
                "1",
                "DSI-1",
                "1920x1200@50.000",
                "0",
                "0",
            ]
        );
    }
}
