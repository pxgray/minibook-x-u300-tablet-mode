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

PCI ID `8086:a7a9` is a general Raptor Lake-U/P integrated GPU ID shared
by many unrelated laptops, not MiniBook-specific. On any other machine
with this exact GPU, this module would also match and force a reprobe of
a possibly-healthy GPU -- another reason it's validated only against this
repo's one documented unit, per `CLAUDE.md`'s single-unit convention.

Same Clang/GCC per-kernel build caveat as `minibook-lid-wake`: this
unit's `cachyos` kernel is Clang-built and needs `LLVM=1`; `cachyos-lts`
is GCC-built and must not get it. `dkms.conf` detects this per kernel
automatically.

## STATUS

Validated on real hardware. Three consecutive full cold boots with
`active=1` (embedded in the initramfs) all reproduced the actual DSI
panel-init bug and cleared it automatically, with no crashes and no
visible corruption observed by the user on any boot. On the
representative boot logged in `docs/findings.md`'s "Intermittent DSI
panel-init failure at boot" entry, the DSI failure-to-completed-reprobe
span took under a second (858ms) -- well under the ~20s a manual
sleep/wake recovery takes. See that entry for the full log evidence.
Longer-term/overnight soak testing is still open.

**Testing note**: this module's real action was validated only via real
early boot, embedded in the initramfs (see Phase 1/Phase 2 below), never
by manually invoking `device_release_driver`/`device_attach` (or an
equivalent manual `i915` sysfs unbind) against a live, in-use desktop
session. An early attempt to do exactly that crashed the kernel and
required a hard reboot -- a known, currently-unfixed upstream DRM/i915
bug where unbinding while a compositor holds open DRM file descriptors
corrupts framebuffer cleanup, not a defect in this module. That sequence
was deliberately never run again after the crash. See `docs/findings.md`
for the full writeup.

## Build and load manually (dry-run smoke test only)

```sh
make LLVM=1          # drop LLVM=1 if your kernel is GCC-built
sudo insmod minibook_dsi_reinit.ko          # dry run, active=0 by default
```

```sh
sudo rmmod minibook_dsi_reinit
```

This only confirms the module loads and that `dmesg` shows the
already-bound-at-init check and/or the live PCI bus notifier firing
correctly, with the real GPU action never invoked (`active=0` is the
default). Do not load this way with `active=1`: per the STATUS section's
testing note above, the real action is only ever exercised via real early
boot through the initramfs (Phase 1/Phase 2 below), never by hand against
a running desktop session.

## Install permanently via DKMS (survives kernel upgrades)

```sh
sudo mkdir -p /usr/src/minibook-dsi-reinit-0.1
sudo cp minibook_dsi_reinit.c Kbuild Makefile dkms.conf /usr/src/minibook-dsi-reinit-0.1/
sudo dkms add -m minibook-dsi-reinit -v 0.1
sudo dkms build -m minibook-dsi-reinit -v 0.1
sudo dkms install -m minibook-dsi-reinit -v 0.1
```

## Phase 1: dry-run validation via initramfs (`active=0`)

This is how the module was actually validated, and the only way it should
be: real early boot, through the same initramfs path it needs to work at
all, never a manual `insmod`/unbind against a live desktop.

```sh
# /etc/mkinitcpio.conf
MODULES=(... minibook_dsi_reinit)
```

```sh
# /etc/modprobe.d/minibook-dsi-reinit.conf
options minibook_dsi_reinit active=0
```

```sh
sudo mkinitcpio -P
```

Reboot (a real cold boot, not a soft reboot) and check `journalctl -k -b`
for the module loading, the already-bound-at-init check or live PCI bus
notifier firing, and the delayed `dry run: would force
device_release_driver + device_attach now` log line. This is exactly what
was run and confirmed working before the real action was ever enabled
(see `docs/findings.md`, finding 11).

## Phase 2: enable the real action (`active=1`, via initramfs)

Only after Phase 1 is trusted, flip `active` to `1` in the same
`modprobe.d` file and rebuild the initramfs:

```sh
# /etc/modprobe.d/minibook-dsi-reinit.conf
options minibook_dsi_reinit active=1
```

```sh
sudo mkinitcpio -P
```

Reboot (several full cold boots, not soft reboots) and check whether the
corruption still appears. This, too, was done only via real cold boots
through the initramfs, never by manually loading `active=1` against an
already-running desktop session -- see the STATUS section above for why.
Three consecutive clean cold boots is what this repo's docs currently
rest on.

## Rollback

Force dry-run mode from the GRUB boot menu without touching graphics at
all: append `minibook_dsi_reinit.active=0` to the kernel command line. To
remove entirely, drop `minibook_dsi_reinit` from `MODULES=()` and
`sudo mkinitcpio -P` again (from a recovery/live USB if the system won't
boot far enough to do it normally).

Writing to `/sys/module/minibook_dsi_reinit/parameters/active` after boot
has no effect: the one-shot guard has already latched (or not) by the
time anyone could reach a shell to write it. The kernel command line
override above, applied at boot, is the only way to actually control
this module's behavior -- a runtime sysfs write is not.
