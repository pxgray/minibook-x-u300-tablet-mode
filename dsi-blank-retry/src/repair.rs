use std::io;
use std::process::Command;

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
}
