# dsi-blank-retry

Userspace daemon that watches for the MiniBook X U300's runtime DSI panel
corruption (`[drm] *ERROR* DSI link not ready`, appearing when GNOME's
idle screen-blank comes back) and automatically retries GNOME's own
`PowerSaveMode` toggle until it clears. This is the runtime counterpart to
[`kernel/minibook-dsi-reinit/`](../kernel/minibook-dsi-reinit/), which
fixes the same failure at boot: that module's trigger is a one-shot guard
on `i915`'s single bind event, so it cannot fire again later. See
[`docs/findings.md`](../docs/findings.md), finding 12, for the evidence
this rests on.

## How it works

It runs as a root systemd service (reading `/dev/kmsg` needs `CAP_SYSLOG`;
`dmesg_restrict` is 1 on this unit). On each `DSI link not ready` line it
sets Mutter's `PowerSaveMode` off then on, as the desktop user via `runuser
-u pxgray -- busctl` (Mutter's `DisplayConfig` interface is on that user's
session bus, which rejects root; `minibookd` uses the same workaround),
then waits 2 seconds for a fresh error. A new error means it did not
clear, so it retries; after 5 attempts it logs a warning and goes back to
watching. It never retries forever. The user name and session bus path are
hardcoded for this one unit.

## STATUS

Validated on real hardware against an induced trigger, not yet against a
natural idle-blank. In 20 induced blank/unblank cycles with the repair
enabled, all 9 detected failures cleared on the first attempt with no
corruption visible on screen. The retry-up-to-5 path is covered by unit
tests but has not been exercised on hardware, and nine events in one
session is a small sample. See `docs/findings.md`, finding 12.

Gated behind the `DSI_BLANK_RETRY_ACTIVE` environment variable: unset or
anything other than `1` means detect-and-log only (the real toggle is
never called).

## Build

```sh
cd dsi-blank-retry
cargo build --release
cargo test
```

## Install

From the repository root:

```sh
sudo install -Dm755 dsi-blank-retry/target/release/dsi-blank-retry /usr/local/bin/dsi-blank-retry
sudo cp systemd/dsi-blank-retry.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now dsi-blank-retry
```

It starts in detect-and-log-only mode (the unit sets
`DSI_BLANK_RETRY_ACTIVE=0`): `journalctl -u dsi-blank-retry` shows a
`DSI error detected` and `dry-run` line per failure, and the toggle is
never called. To enable the real repair, add a drop-in and restart:

```sh
sudo mkdir -p /etc/systemd/system/dsi-blank-retry.service.d
printf '[Service]\nEnvironment=DSI_BLANK_RETRY_ACTIVE=1\n' | sudo tee /etc/systemd/system/dsi-blank-retry.service.d/override.conf
sudo systemctl daemon-reload
sudo systemctl restart dsi-blank-retry
```

The journal should then show `starting (active=true)`, and each detection
is followed by `cleared after N attempt(s)` or `gave up after 5 attempts`.

## Rollback

To keep the daemon running but stop the real repair, remove the drop-in:

```sh
sudo rm /etc/systemd/system/dsi-blank-retry.service.d/override.conf
sudo systemctl daemon-reload
sudo systemctl restart dsi-blank-retry
```

To remove it entirely:

```sh
sudo systemctl disable --now dsi-blank-retry
```
