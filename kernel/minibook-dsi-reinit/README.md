# minibook-dsi-reinit

Out-of-tree kernel module that forces the MiniBook X U300's `i915` GPU
through one driver remove+probe cycle as early in boot as possible, to
work around an intermittent DSI panel-init failure (`[drm] *ERROR* DSI
link not ready`) confirmed on this exact unit. A shallow sleep/wake cycle
has always cleared the corruption by hand; this module automates that
same recovery via `device_release_driver()`/`device_attach()` on the GPU
device, rather than patching `i915`'s internal (unexported) DSI init
code. See
[`docs/findings.md`](../../docs/findings.md) and the design spec at
[`docs/superpowers/specs/2026-09-19-dsi-reinit-fix-design.md`](../../docs/superpowers/specs/2026-09-19-dsi-reinit-fix-design.md)
for the full root-cause evidence and rationale.

## Hardware/software baseline

Built and validated only against this repo's one documented unit: GPU PCI
ID `8086:a7a9` at `0000:00:02.0` (confirmed via `lspci -nn`), kernel
`7.2.6-1-cachyos`. `i915` loads from the initramfs via `mkinitcpio`'s
`kms` hook on this unit, which is why this module must also be embedded
in the initramfs (see Phase 2 below) rather than loaded the normal way
via `/etc/modules-load.d` -- that only loads after `i915` has already
bound, too late to catch it.

Same Clang/GCC per-kernel build caveat as `minibook-lid-wake`: this
unit's `cachyos` kernel is Clang-built and needs `LLVM=1`; `cachyos-lts`
is GCC-built and must not get it. `dkms.conf` detects this per kernel
automatically.

## STATUS

Validated on real hardware. Three consecutive full cold boots with
`active=1` (embedded in the initramfs) all reproduced the actual DSI
panel-init bug and cleared it automatically within ~250-350ms of the
trigger firing, with no crashes and no visible corruption observed by the
user on any boot. See `docs/findings.md`'s "Intermittent DSI panel-init
failure at boot" entry for the full log evidence. Longer-term/overnight
soak testing is still open.

**Testing note**: validate this module's real action only via a real
early boot (as done above), never by manually invoking
`device_release_driver`/`device_attach` (or an equivalent manual `i915`
sysfs unbind) against a live, in-use desktop session. An early attempt to
do exactly that crashed the kernel and required a hard reboot -- a known,
currently-unfixed upstream DRM/i915 bug where unbinding while a
compositor holds open DRM file descriptors corrupts framebuffer cleanup,
not a defect in this module. See `docs/findings.md` for the full
writeup.

## Build and load manually (for testing)

```sh
make LLVM=1          # drop LLVM=1 if your kernel is GCC-built
sudo insmod minibook_dsi_reinit.ko          # dry run, active=0 by default
sudo insmod minibook_dsi_reinit.ko active=1 # performs the real reprobe
```

```sh
sudo rmmod minibook_dsi_reinit
```

## Install permanently via DKMS (survives kernel upgrades)

```sh
sudo mkdir -p /usr/src/minibook-dsi-reinit-0.1
sudo cp minibook_dsi_reinit.c Kbuild Makefile dkms.conf /usr/src/minibook-dsi-reinit-0.1/
sudo dkms add -m minibook-dsi-reinit -v 0.1
sudo dkms build -m minibook-dsi-reinit -v 0.1
sudo dkms install -m minibook-dsi-reinit -v 0.1
```

## Phase 1: validate the trigger logic (dry run, `active=0`)

Load manually as above (no initramfs changes needed yet) and confirm via
`dmesg` that the already-bound check and the live PCI bus notifier both
fire correctly and the one-shot guard prevents double-firing. See the
design spec's Testing section for the exact manual unbind/rebind
sequence used to exercise the live-notifier path in isolation.

## Phase 2: enable it at boot (`active=1`, via initramfs)

Only after Phase 1 and a manual `active=1` test (against a known-good,
non-corrupted display) are both trusted:

```sh
# /etc/mkinitcpio.conf
MODULES=(... minibook_dsi_reinit)
```

```sh
# /etc/modprobe.d/minibook-dsi-reinit.conf
options minibook_dsi_reinit active=1
```

```sh
sudo mkinitcpio -P
```

Reboot (several full cold boots, not soft reboots) and check whether the
corruption still appears.

## Rollback

Force dry-run mode from the GRUB boot menu without touching graphics at
all: append `minibook_dsi_reinit.active=0` to the kernel command line. To
remove entirely, drop `minibook_dsi_reinit` from `MODULES=()` and
`sudo mkinitcpio -P` again (from a recovery/live USB if the system won't
boot far enough to do it normally).
