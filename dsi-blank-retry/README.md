# dsi-blank-retry

Userspace daemon that watches for the MiniBook X U300's runtime DSI panel
corruption (`[drm] *ERROR* DSI link not ready`, appearing when GNOME's
idle screen-blank comes back, or left over from boot) and automatically retries GNOME's own
`PowerSaveMode` toggle until it clears. This is the runtime counterpart to
[`kernel/minibook-dsi-reinit/`](../kernel/minibook-dsi-reinit/), which
fixes the same failure at boot: that module's trigger is a one-shot guard
on `i915`'s single bind event, so it cannot fire again later. See
[`docs/findings.md`](../docs/findings.md), finding 12, for the evidence
this rests on.

## How it works

It runs as a root systemd service (reading `/dev/kmsg` needs `CAP_SYSLOG`;
`dmesg_restrict` is 1 on this unit). On each `DSI link not ready` line it
sets Mutter's `PowerSaveMode` off then on, then waits 2 seconds for a
fresh error. A new error means it did not clear, so it retries; after 5
attempts it logs a warning and goes back to watching. It never retries
forever.

The toggle runs as whoever owns the display: the user of `seat0`'s active
logind session, looked up again on every attempt, via `setpriv
--reuid=<uid> --regid=<gid> --clear-groups -- busctl` against
`/run/user/<uid>/bus` (Mutter's `DisplayConfig` interface is on that
user's session bus, which rejects root). Before login that is GDM's
greeter, a dynamic user (`gdm-greeter`) with no permanent passwd entry,
which is why it switches by numeric uid rather than `runuser -u <name>`.

It also covers failures from before it started. At startup it reads the
kernel log backlog and repairs straight away if the last `DSI link not
ready` in it comes after the last `reprobe complete` (from
`kernel/minibook-dsi-reinit/`) and the last `PM: suspend exit`, since
either of those re-enables the panel. That catches the boots where the
module's own reprobe fails (see `docs/findings.md`, finding 11), which
happens a few seconds before this service starts. A restart mid-session
can repeat a repair for an error that was already cleared, costing one
extra blank/unblank.

## STATUS

Validated on real hardware against an induced trigger, not yet against a
natural idle-blank. In 20 induced blank/unblank cycles with the repair
enabled, all 9 detected failures cleared on the first attempt with no
corruption visible on screen. The retry-up-to-5 path is covered by unit
tests but has not been exercised on hardware, and nine events in one
session is a small sample. See `docs/findings.md`, finding 12.

The startup backlog repair through GDM's greeter session has worked on
one real boot so far (2026-09-24 19:45). The reprobe failed at 5.3 s,
the daemon started at 12.9 s, found the error and reported it cleared
after 1 attempt at 15.2 s. The user saw the corruption clear once GDM
started. Before that, the startup rule was replayed offline against the
kernel logs of the 11 earlier boots in the journal. It flagged exactly
the 5 boots left with an uncleared failure.

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
