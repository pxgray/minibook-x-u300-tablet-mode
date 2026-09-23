# dsi-blank-retry

Userspace daemon that watches for the runtime (idle-blank-triggered) DSI
panel corruption and automatically retries GNOME's own `PowerSaveMode`
toggle until it clears. Unlike
[`kernel/minibook-dsi-reinit/`](../kernel/minibook-dsi-reinit/), which
fixes the same underlying DSI panel-init race at boot, this covers the
case where `minibook-dsi-reinit`'s one-shot boot trigger never fires:
GNOME blanking the screen after idle (this unit runs with
`sleep-inactive-ac-type=nothing`, so idle only blanks the display on AC
power, it never suspends) and then unblanking it.

## STATUS

Untested prototype. Gated behind the `DSI_BLANK_RETRY_ACTIVE` environment
variable (default off / detect-and-log only) until validated on real
hardware.

## Build

```sh
cd dsi-blank-retry
cargo build --release
```

## Install

```sh
sudo install -Dm755 target/release/dsi-blank-retry /usr/local/bin/dsi-blank-retry
sudo cp ../systemd/dsi-blank-retry.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now dsi-blank-retry
```

Starts in dry-run mode (`DSI_BLANK_RETRY_ACTIVE=0`, the unit file's
default) -- it will log detected DSI errors via `journalctl -u
dsi-blank-retry` but never actually call the repair toggle. Enable the
real action only after that's been validated:

```sh
sudo systemctl edit dsi-blank-retry
```

Add:
```ini
[Service]
Environment=DSI_BLANK_RETRY_ACTIVE=1
```

```sh
sudo systemctl restart dsi-blank-retry
```

## Rollback

```sh
sudo systemctl disable --now dsi-blank-retry
```

Or, to keep it running but stop the real action:
```sh
sudo systemctl edit dsi-blank-retry
```
and set `Environment=DSI_BLANK_RETRY_ACTIVE=0`, then
`sudo systemctl restart dsi-blank-retry`.
