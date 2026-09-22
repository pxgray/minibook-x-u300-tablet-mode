# Findings

Reverse-engineering methodology and evidence behind
[minibookd](../README.md)'s tablet-mode support: why this approach was
chosen over existing community projects, the hardware/software baseline
it was investigated against, how this board revision's ACPI differs from
previously-documented units, and the full record of empirical validation
each design decision is based on.

See the [README](../README.md) for installing and using the tool; this
document is the research record its "Findings" section summarizes, and
the evidence [`CLAUDE.md`](../CLAUDE.md)'s editorial standard requires
every factual claim about this hardware to trace back to.

## Motivation

Existing community approaches to tablet mode on the MiniBook X fall into two
camps:

1. **Userspace input interception** ([lschans/chuwi-tablet](https://github.com/lschans/chuwi-tablet)):
   grabs the keyboard's evdev device with `evtest --grab` to swallow its
   events in tablet mode. GNOME-specific, requires installing a GNOME Shell
   extension into both the user's and GDM's profiles, and doesn't use the
   accelerometers at all (manual toggle only).
2. **From-scratch platform driver** ([rhalkyard](https://github.com/rhalkyard/minibook-dual-accelerometer) /
   [bazmonk](https://github.com/bazmonk/minibook-dual-accelerometer) forks,
   [greymouser/minibook-x-tools](https://github.com/greymouser/minibook-x-tools)):
   a custom out-of-tree kernel module plus 2-3 userspace daemons, essentially
   reimplementing a chunk of platform/HID plumbing from scratch.

Both exist because the MiniBook X's ACPI firmware only auto-enumerates one of
its two accelerometers, and (on older units) getting a `SW_TABLET_MODE`
switch to GNOME requires either patching `intel-hid`'s DMI allow-list or
inventing a new signaling path.

This project investigates a third option: **reuse the vendor firmware's own
tablet-mode switching method** via the generic, already-packaged `acpi_call`
kernel module, rather than writing bespoke kernel code. On older units, this
works close to out-of-the-box. On the U300, it partially doesn't (discussed
below).

## Hardware/software baseline

- CPU: Intel Core i3-U300
- Tested kernel: CachyOS custom kernel (`linux-cachyos`), version `7.2.4-3-cachyos`
- `chassis_type` (`/sys/class/dmi/id/chassis_type`) reads **`31`** (Convertible)
  out of the box, which is better than the `10` (Notebook) reported by older-unit
  owners in the Chuwi forums, suggesting this DMI table is more correct on
  this revision.
- `intel_hid` is loaded by default.
- Only one of the two accelerometers auto-enumerates at boot
  (`iio:device0`, driver `mxc4005`, ACPI companion `MDA6655:00`).
- Boot kernel command line includes `video=DSI-1:panel_orientation=right_side_up`
  and `fbcon=rotate:1` (confirmed via `/proc/cmdline` on this unit), correcting
  the DSI panel's native portrait orientation for the console and for
  Wayland/DRM. This matches the
  [ArchWiki's Chuwi MiniBook X (2023) page](https://wiki.archlinux.org/title/Chuwi_MiniBook_X_(2023)#Screen_rotation),
  which documents that this hardware's screen is rotated by default on
  exiting the BIOS, and points to
  [Tablet PC#Screen rotation](https://wiki.archlinux.org/title/Tablet_PC#Screen_rotation)
  for the fix, where `panel_orientation` is the documented kernel directive
  for Wayland/DRM consumers. That ArchWiki page targets the 2023 (N100)
  model rather than this U300 unit; only the `/proc/cmdline` check above is
  specific to this unit, the rest is carried over as documented, not
  independently re-derived.

## What's different about the U300 vs. documented units

All of this was determined by dumping and disassembling the live DSDT
(`sudo acpidump -b -n DSDT`, then `iasl -d`, both via the `acpica` package;
no vendor documentation was available):

| | Older units (N100/N150, per rhalkyard/bazmonk) | U300 |
|---|---|---|
| Accelerometer ACPI device name | `ACMK` | `ACMG` |
| Full ACPI path | `\_SB.ACMK` | `\_SB.PC00.I2C1.ACMG` |
| I2C buses used | `I2C0` (base) + `I2C1` (display) | `I2C1` (display) + `I2C3` (base) |
| Runtime tablet-mode signal | `Notify (HIDD, 0xCD)` / `Notify (HIDD, 0xCC)` (a real ACPI notification) | `SPC0 (0x090E000C, ...)`, a raw GPIO pad-config MMIO write with **no `Notify` at all** |
| EC keyboard-disable | `^^PC00.LPCB.H_EC.KBCD` direct field write | `^^^LPCB.H_EC.ECWT(..., RefOf(...KBCD))` (same register, via a wrapper method) |

The `ACMG.LTSM` method on this unit:

```asl
Method (LTSM, 1, NotSerialized)
{
    If ((Arg0 == Zero))
    {
        IDX1 = 0x10
        DTA1 = Zero
        ^^^LPCB.H_EC.ECWT (Zero, RefOf (^^^LPCB.H_EC.KBCD))
        SPC0 (0x090E000C, 0x40900102)
    }
    ElseIf ((Arg0 == One))
    {
        IDX1 = 0x10
        DTA1 = 0xFF
        ^^^LPCB.H_EC.ECWT (0x03, RefOf (^^^LPCB.H_EC.KBCD))
        SPC0 (0x090E000C, 0x44000200)
    }
}
```

`SPC0`/`GPC0` are generic helpers elsewhere in the DSDT that decode their
first argument via `GGRP`/`GNMB` (`(Arg0 & 0x00FF0000) >> 16` = GPIO group,
`Arg0 & 0xFFFF` = pad number) into a GPIO controller MMIO address via
`GADR`/`SBRG`/`GINF`, then write the second argument as a raw `PADCFG0`
register value. `0x090E000C` decodes to **GPIO group `0x09`, pad `0x0C`**.
This is a direct pad-config write, entirely bypassing `gpiolib` (it doesn't
appear in `/sys/kernel/debug/gpio`) and, empirically, bypassing Linux's ACPI
event model too (see below). It is not the same mechanism as the
`Notify(HIDD, ...)` call older units use, which is what `intel-hid`'s
`notify_handler()` (gated by `dmi_vgbs_allow_list` in
`drivers/platform/x86/intel/hid.c`) is built to catch.

## Empirical validation

### 1. `acpi_call` + `LTSM` disables EC keyboard and touchpad (confirmed working)

Installed `acpi_call-dkms` (official Arch `extra` repo, builds cleanly via
DKMS against `linux-cachyos-headers`) and called the method with
[`scripts/test-ltsm-switch.sh`](../scripts/test-ltsm-switch.sh), which:

1. Queues a backgrounded, timed auto-revert (`LTSM 0x0` after 5s) **before**
   ever entering tablet mode, so the keyboard un-disables itself
   automatically regardless of what else happens. Don't call this method
   blind without that safety net: it disables your only input device
   immediately.
2. Calls `LTSM 0x1` (tablet mode) via `/proc/acpi/call`.
3. Captures events on the Intel HID input device and recent `dmesg` output
   during the tablet-mode window.

```sh
./scripts/test-ltsm-switch.sh
```

Results:

- The call returns `0x44000200`, the exact literal `SPC0` payload from the
  `Arg0==1` branch (ACPICA implicitly returns a method's last-evaluated value
  when there's no explicit `Return`), confirming the correct branch executed
  with no ACPI error.
- **Both the physical keyboard and the touchpad stopped responding**
  for the duration of tablet mode, confirmed directly at the hardware. The
  single `KBCD` EC register write disables both, not just the keyboard
  despite the register's name, so there's no touchpad-based fallback (e.g.
  clicking an on-screen keyboard) if a revert somehow fails; only an
  external USB/Bluetooth input device or a hard reboot would work as a
  backup. `test-ltsm-switch.sh`'s auto-revert safety net accounts for this.

### 2. Runtime tablet-mode signal: confirmed NOT reaching Linux, on any checked path

The natural first guess is that this is a `dmi_vgbs_allow_list` gating issue
in `intel_hid` (the mechanism older, non-allowlisted Intel platforms hit,
where the notification arrives but gets reported as `KEY_UNKNOWN` instead of
a real switch). That turned out not to be broad enough a check on its own,
so this was tested at several levels, from the specific driver outward to
the raw ACPI interrupt layer:

- **`evtest` on `/dev/input/eventN`** ("Intel HID events", ACPI companion
  `INTC1078`, capability mask `EV=13` = `SYN`+`KEY`+`MSC` only, with **no
  `SW` capability bit at all**) while `LTSM(1)` is active: zero events,
  not even `KEY_UNKNOWN`.
- **`dmesg`** during the same window: nothing.
- **`intel_vbtn`**, a separate driver from `intel_hid` that also handles
  `SW_TABLET_MODE` on some platforms: not loaded at all on this system
  (`lsmod` shows nothing), ruling out that alternate path outright rather
  than just deprioritizing it.
- **Raw ACPI GPE interrupt counters** (`/sys/firmware/acpi/interrupts/gpe*`),
  checked before and after independent of any specific driver: of the seven
  GPEs that were enabled and sitting at `0`, all seven were still at exactly
  `0` after the call. One separate, already-active GPE incremented, but at
  its ordinary background rate, unrelated to the call's timing. So the
  `SPC0` write doesn't trigger any ACPI-level interrupt on this system at
  all, not just one that a particular driver fails to interpret.
- **`net.hadess.SensorProxy`** (the actual D-Bus service `iio-sensor-proxy`
  exposes, and what GNOME Shell/`gnome-settings-daemon` really consume):
  queried live via `gdbus` before and during tablet mode. The property set
  is identical both times, and `TabletMode` never appears in it at all, not
  even as `false`.
- No `hidraw` device is relevant either; the two present belong to the USB
  mouse and the touchpad.

**Result: nothing, on every path checked.** This rules out "just patch the
DMI allow-list" as a fix on this unit: there's no notification reaching
`intel_hid` (or any other loaded driver, or the ACPI interrupt layer
directly) to allow-list in the first place. Whatever the `SPC0` GPIO write
does, it produces no effect visible to current mainline Linux through any
surface checked here.

**Practical conclusion:** on the U300, firmware will not tell GNOME about
tablet mode for free. A userspace daemon needs to synthesize a
`SW_TABLET_MODE` switch itself (e.g. via `/dev/uinput`) alongside calling
`LTSM` for the keyboard/touchpad. That's a much narrower scope than the
existing projects' virtual keyboard/touchpad passthrough devices (one
boolean switch, not full input interception), and still no `intel-hid`
kernel patching required since we no longer depend on it at all.

### 3. Second accelerometer bring-up

ACPI `\_SB.PC00.I2Cn` maps 1:1 to physical adapter `/sys/bus/i2c/devices/i2c-n`
on this SoC (confirmed via direct sysfs device-tree inspection, not
assumption): `i2c_designware.0`-`.3` sit at PCI `00:15.0`-`.3`,
`i2c_designware.4`-`.5` at PCI `00:19.0`-`.1`. So:

- `I2C1` = `/sys/bus/i2c/devices/i2c-1`, which already hosts the display
  accelerometer (`i2c-MDA6655:00`, `iio:device0`).
- `I2C3` = `/sys/bus/i2c/devices/i2c-3`, the base accelerometer's bus,
  empty by default.

```sh
./scripts/add-second-accelerometer.sh
```

brings it up as `iio:device1` (confirmed via `dmesg`: `i2c i2c-3:
new_device: Instantiated device mxc4005 at 0x15`).

To make this survive reboot,
[`udev/61-minibook-accelerometer.rules`](../udev/61-minibook-accelerometer.rules)
triggers the same instantiation automatically off the display
accelerometer's `iio:device0` appearing (**confirmed working**: `iio:device1`
comes up on its own after a reboot, with no manual step), adapted from
[rhalkyard's `60-sensor-chuwi.rules`](https://github.com/rhalkyard/minibook-dual-accelerometer/blob/main/hack-driver/60-sensor-chuwi.rules)
but pointed at `i2c-3` instead of `i2c-0`, and deliberately without setting
`ACCEL_MOUNT_MATRIX` (not yet determined on this board) or handing off to a
daemon service (that's `minibookd`, added later). Install it with:

```sh
sudo cp udev/61-minibook-accelerometer.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules
sudo udevadm trigger
```

then reboot and confirm `iio:device1` appears without running
`add-second-accelerometer.sh` manually.

### 4. Hinge-angle algorithm: validated against real hardware, zero calibration

Took raw accelerometer readings at three known hinge positions (both sensors
share `in_accel_scale = 0.009582`) and computed the angle between the two raw
vectors via `arccos((v1·v2)/(|v1||v2|))`, the same approach used by
rhalkyard/bazmonk's `angle-sensor.py`, deliberately with **no mount-matrix
correction**, to see whether the core algorithm holds up at all on this
board. [`scripts/vector_angle.py`](../scripts/vector_angle.py) does this
computation; hold the laptop at a given hinge angle and run it to reproduce
a row of the table below:

```sh
./scripts/vector_angle.py
```

| Position | display raw (x,y,z) | base raw (x,y,z) | expected angle | computed angle |
|---|---|---|---|---|
| Typing angle, base flat on table | (-787, 36, 450) | (-3, 8, -1632) | ~110-120° | **119.6°** |
| Flat open (whole unit flat on table) | (3, 21, 809) | (13, 2, -1620) | 180° | **178.3°** |
| Folded tablet (base flat, screen up) | (3, 26, 793) | (2, -6, 426) | 0°/360° | **2.7°** |

Near-exact match at every position with zero calibration. This validates
that dot-product/`arccos` between the raw vectors is sufficient for hinge
angle / tablet-mode detection on this hardware (a proper mount matrix would
still be needed for absolute screen auto-rotation, which needs true "which
way is up" for the display, but not for detecting the hinge angle itself).

The base sensor's reading is consistent across the first two positions
(`z` ≈ -1632, -1620) since the base itself didn't move; it only changes when
the whole assembly is physically picked up and reoriented (as naturally
happens when someone actually folds a laptop into tablet form to hold it),
which the relative-angle math is unaffected by.

Both sensors also show the same **~6 m/s² Z-axis DC offset** quirk that
rhalkyard and bazmonk documented on their
units; the base sensor reads a magnitude of ~15.5 m/s² when only
gravity (~9.8 m/s²) should be present. This appears to be a characteristic
of the MXC4005/MXC6655 MEMS chip itself, not something specific to older
board revisions.

### 5. Hinge-angle calibration: sensor offset correction, signed tilt-corrected angle

The zero-calibration formula above (finding 4) was validated only with the
laptop resting flat on a desk. Real-world `--dry-run` testing surfaced two
problems with it away from a desk:

- **Whole-body tilt corrupts the measured angle.** Two readings taken at
  the same physical hinge fold (hinge confirmed unmoved) but different
  whole-body posture (lying down vs. sitting up, laptop on legs in both
  cases) produced measured angles 36 degrees apart with the desk-only
  formula.
- **`arccos` cannot express fold direction.** Bounded to `[0, 180]`, it
  can't distinguish a forward recline from a backward tent/presentation
  fold, since both can produce the same unsigned angle.

Both trace to the same root cause: the formula only measures the angle
*between* the two raw vectors, with no way to correct for whole-body tilt
or recover a signed direction. Fixing this needed two independent
corrections, both fitted from 18 real readings spanning desk, tent,
presentation, and reclined-lap postures at varying whole-body tilt (see
[`scripts/calibration_data.json`](../scripts/calibration_data.json)):

**Sensor Z-axis offset.** The ~6 m/s² DC offset noted in finding 4 isn't
just a magnitude curiosity: left uncorrected, it distorts the *direction*
computed from the raw vectors too, and the distortion grows with tilt.
[`scripts/calibrate_hinge_axis.py`](../scripts/calibrate_hinge_axis.py) fits
a per-sensor additive Z-axis offset via a sphere fit (leave-one-out
stability under 1.2% across all 18 readings, confirming a real, stable
constant rather than a fitting artifact): base sensor -603.6 raw counts
(-5.78 m/s²), display sensor: -178.4 raw counts (-1.71 m/s²). Both are
close to independently-measured values in rhalkyard's own README on a
*different* unit ("approximately -6 [m/s²]" on the Z axis), corroborating
this as a real characteristic of the sensor family rather than
unit-specific noise. Only Z is corrected; a small X-axis component
marginally reduced residual in testing but isn't corroborated by any
prior art and was deliberately not used, to avoid fitting noise on a
weakly-sampled axis.

**Hinge axis and sensor-mounting rotation.** With the offset corrected,
the same script fits the physical hinge axis (expressed in the base
sensor's raw frame) and the fixed rotation between the two sensors'
mounting orientations, via nonlinear least-squares. Readings taken with
the hinge confirmed unmoved between captures (marked with a shared
`group` in the data file) become equality constraints in the fit, so no
protractor is needed anywhere in this process. The result lets
`daemon/src/angle.rs`'s `signed_hinge_angle` compute a **signed** angle
via `atan2` instead of `arccos`, resolving fold direction, on top of the
offset correction.

**Validation.** Every one of the 18 readings' expected classification
(Laptop vs. Tablet) is checked directly in both
[`scripts/calibrate_hinge_axis.py`](../scripts/calibrate_hinge_axis.py)'s
output and `daemon/src/angle.rs`/`state.rs`'s test suites. Laptop-expected
readings cluster in `[45.1, 110.4]` degrees; Tablet-expected readings sit
outside that on both sides (as low as -144.1, as high as 149.0), a
comfortable margin given the daemon only needs a binary split, not
research-grade absolute-angle precision.

To reproduce or extend this calibration:

```sh
python3 scripts/calibrate_hinge_axis.py --self-test  # synthetic, no hardware needed
python3 scripts/calibrate_hinge_axis.py              # real fit against calibration_data.json
```

To add more data, append readings to `calibration_data.json` (`group` set
to a shared name for any two readings taken with the hinge confirmed
unmoved between them, `null` otherwise), rerun the script, and update
`BASE_OFFSET`/`DISPLAY_OFFSET`/`HINGE_AXIS`/`MOUNT_ROTATION` in
`daemon/src/angle.rs` with the new fitted values.

**Open question, not fully resolved:** a smaller (~10-15 degree) residual
disagreement remains between some same-hinge-fold reading pairs; cause
unidentified. It doesn't appear to block practical classification (see the
clean Laptop/Tablet separation above) and did not surface as a problem
during real-hardware validation (finding 7), but a contributor extending
this calibration should know it's there rather than assume the fit is exact.

### 6. Base-tilt gate: distinguishing a closing lid from a folded-and-picked-up tablet

Real-hardware `--dry-run` testing after finding 5 surfaced a false
positive: an "almost closed" lid, resting normally on a surface (not
picked up), triggered Tablet mode. A real reading from that position:

```
display (iio:device0): (-509, -6, -1058)
base    (iio:device1):    (10, -9, -1634)
angle between vectors:   26.0 deg
```

This computes to a signed angle of -39.9 degrees under the finding-5
calibration, past `TABLET_ENTER_LOW`, so the low-side Tablet trigger fired
correctly by its own logic. The problem is that low signed angle alone
can't distinguish this from a genuine tablet fold: both a closing lid and
a screen folded flat and picked up pass through similar angle values, and
the calibration data already contains real Tablet-expected readings on
this same low/negative side (`folded_tablet_desk`, `self_standing_tent`,
`presentation_flipped`), so the threshold can't simply be moved without
breaking those.

What actually distinguishes them is the base sensor's own orientation.
Computing the angle between the base's raw vector and the "resting
normally" reference direction (`angle::base_tilt_from_level`) across all
18 calibration readings:

| Case | Signed angle | Base tilt from level |
|---|---|---|
| All ordinary desk-use readings | 57.98 to 110.4 | 0.4-0.7 degrees |
| All ordinary lap-use readings (reclining tilts the whole unit, sometimes substantially) | 45.1 to 69.9 | 3.5-55.9 degrees |
| The false-positive "almost closed" reading above | -39.9 | 0.75 degrees |
| `self_standing_tent` (low-side Tablet, gated) | -124.3 | 62.6 degrees |
| `folded_tablet_desk` / `presentation_flipped` (low-side Tablet, gated, flipped) | -70.4 / -144.1 | 179.7 / 178.8 degrees |
| `hand_held_tent` (high-side Tablet, ungated by this check) | 149.0 | 18.4 degrees |

The lap-use readings' base tilt overlaps with the genuine Tablet readings'
range (both reach well past 8 degrees), which looks concerning at first,
but doesn't matter in practice: every lap-use reading's *signed angle*
stays at 45.1 or above, far short of `TABLET_ENTER_LOW` (20), so none of
them ever reach the gated check regardless of their own tilt. The gate
only has to separate cases that already have a low angle, and there the
separation is clean: the false positive sits at 0.75 degrees, the nearest
genuine low-side Tablet reading at 62.6, a wide margin either side of the
chosen 8-degree threshold.

`daemon/src/state.rs`'s low-side Tablet entry now additionally requires
`angle::base_tilt_from_level(base) > BASE_TILT_THRESHOLD` (8 degrees),
verified against all 18 calibration readings plus the specific
false-positive reading above. The high-side (tent/presentation) entry is
intentionally left ungated, since it was confirmed correctly classifying
without this check.

### 7. Daemon validated end-to-end on real hardware; three bugs found and fixed

Running `minibookd` live (not `--dry-run`) surfaced three real bugs, each
diagnosed and fixed against the actual failure it caused:

- **`/proc/acpi/call`'s response includes a trailing NUL byte** that
  `str::trim()` doesn't strip (`\0` isn't Unicode whitespace), which broke
  parsing of the startup reconciliation's `LTSM(0)` response
  (`acpi::parse_response`). Fixed by trimming NUL alongside whitespace.
- **No `SIGINT`/`SIGTERM` handler.** Killing the daemon with Ctrl+C while
  a transition to Tablet had fired left the keyboard and touchpad
  disabled with no in-process recovery -- confirmed directly: after one
  such kill, neither the internal keyboard, the touchpad, nor a plugged-in
  USB keyboard responded, and only a full power cycle recovered input.
  Root-caused to this rather than a vendor firmware recovery bug by
  reproducing the same `LTSM(0)` call independently via
  `scripts/test-ltsm-switch.sh`, bypassing the daemon entirely, which
  recovered both keyboard and touchpad cleanly. Fixed with a
  `ctrlc`-based signal handler that reverts to Laptop state before
  exiting, mirroring the existing watchdog's emergency-revert pattern.
- **Unsynchronized concurrent access to `/proc/acpi/call`.** The main poll
  loop, the watchdog thread, and (once added) the signal handler could
  all call `acpi::set_tablet_mode` independently, with no serialization
  around the shared, stateful write-then-read protocol that file expects.
  Fixed with a module-level `Mutex` in `acpi.rs` serializing every call.
- **The `uinput` `SW_TABLET_MODE` switch was never cleared before the
  process exited on a signal**, only destroyed outright when its file
  descriptor closed. GNOME had already been observed reacting broadly to
  the switch (auto-rotate and the on-screen keyboard both engage
  correctly while the daemon signals Tablet mode); losing the device
  without an explicit `SW_TABLET_MODE=0` event first appears to be why
  the keyboard/touchpad failure above also affected an external USB
  keyboard, not just the EC-disabled internal one -- consistent with a
  compositor-level input policy, not just the EC register. Fixed by
  clearing the switch (in addition to the ACPI revert) on every exit
  path: the signal handler, the watchdog's emergency revert, and the
  normal `ToLaptop` transition.

After all three fixes, a full live cycle (Laptop to Tablet to Laptop
again, killed with Ctrl+C at various points including mid-Tablet) was
confirmed working: transitions fire correctly in both directions, `LTSM`
disables and re-enables the keyboard and touchpad correctly, and input is
fully restored on exit without a reboot.

One remaining, separate issue observed during this testing: the screen
consistently landed in portrait orientation after the Tablet-to-Laptop
transition, rather than the correct landscape. At the time this looked
like an `ACCEL_MOUNT_MATRIX` problem; it wasn't -- see finding 8, which
identifies and fixes the actual cause.

### 8. Screen stuck in portrait after Tablet-to-Laptop was a Mutter bug, not a sensor-calibration problem

The portrait regression from finding 7 persisted even with no
`ACCEL_MOUNT_MATRIX` set and the daemon running continuously and
uninterrupted under systemd (i.e. with the uinput device never
disappearing and `SW_TABLET_MODE` being cleared normally through
`handle_transition`'s `ToLaptop` branch), which rules out both the sensor
calibration and the daemon's own process lifecycle as the cause.

Queried Mutter's live state directly instead of guessing further:

```sh
busctl --user call org.gnome.Mutter.DisplayConfig \
  /org/gnome/Mutter/DisplayConfig org.gnome.Mutter.DisplayConfig \
  GetCurrentState
```

The single logical monitor's `transform` property was `1` (Mutter's enum
for a 90-degree rotation) instead of `0` (normal), applied on top of
whatever the kernel's `panel_orientation=right_side_up` correction already
establishes as "normal" -- confirmed live: the physical unit was resting
in landscape, `transform` read `1`, and the screen showed portrait.
Mutter's auto-rotate does not reset this transform back to normal on the
`SW_TABLET_MODE` 1-to-0 transition; it leaves whatever rotation was last
live-applied during Tablet mode in place.

Fixed in `daemon/src/display.rs`: `handle_transition`'s `ToLaptop` branch
now also calls Mutter's `ApplyMonitorsConfig` (method `1`, temporary --
doesn't rewrite `monitors.xml`) forcing `transform=0`, confirmed via the
same `GetCurrentState` query to restore the correct landscape orientation.

One complication, also found empirically rather than assumed: the daemon
runs as root (needed for `/proc/acpi/call` and `/dev/uinput`), but
Mutter's `DisplayConfig` interface lives on the logged-in user's session
D-Bus, not root's own. Calling `busctl` directly as root with `--address`
pointed at the user's session socket (`/run/user/1000/bus`) fails with
`Call failed: Transport endpoint is not connected` -- the session bus
authenticates connections by the connecting process's real uid
(`SO_PEERCRED`) and rejects root outright, regardless of which socket path
it connects to. Fixed by wrapping the call in `runuser -u <user> --`, so
`busctl` itself actually runs as the target user rather than as root.

Verified end-to-end via the installed systemd service: a full
Laptop-to-Tablet-to-Laptop cycle now correctly returns to landscape, with
no `failed to reset display rotation` line in `journalctl -u minibookd`.

Caveat: `daemon/src/display.rs` hardcodes this unit's single display's
connector, mode, and scale (consistent with this repo's single-unit
convention, see `CLAUDE.md`) rather than reading them back live each time.
If the display scale is changed in GNOME Settings, the next
Tablet-to-Laptop transition will silently reset it back to `1.25`.

### 9. Suspend/resume reconciliation, confirmed working on real hardware

The daemon's poll loop and watchdog both use a monotonic clock that
doesn't advance across suspend, so a resume was never observed on its own
-- nothing in the running process would notice one happened without an
external poke.

Added `reconcile()` (re-reads the hinge angle and reapplies hardware state
instead of assuming Laptop), called at startup and on `SIGUSR1`, plus a
[`systemd/system-sleep/minibookd`](../systemd/system-sleep/minibookd) hook
that sends that signal on resume.

The specific motivation was an **unverified hypothesis**, not an observed
bug: that the EC might reset its keyboard-disable register (`KBCD`) across
suspend independently of the daemon. That hypothesis remains untested (no
confirmed case of it happening either way).

**The hook mechanism itself is confirmed working** on real hardware: a
real suspend/resume cycle shows `systemd` delivering `SIGUSR1` and the
daemon logging `caught SIGUSR1 (resume), reconciling to current hinge
angle` in `journalctl -u minibookd`, with the service remaining active and
no errors afterward.

### 10. Lid-open couldn't wake the system from suspend; fixed with a GPE force-arm module

**Symptom**, unrelated to `minibookd` or the `LTSM` findings above: closing
the lid suspends the system (pre-existing GNOME/`logind` behavior, nothing
this project changes), but opening it back up does not resume it -- only
touching the keyboard or touchpad does.

**Root cause, confirmed from the live DSDT**: the ACPI Lid device (`LID0`,
`PNP0C0D`) has no `_PRW` (Power Resources for Wake) method and no `_PSW`
(Power State Wake) method -- nothing that gives Linux a way to arm it as a
wake source. This matches two other independent observations on this
unit: `/proc/acpi/wakeup` lists no `LID0` entry at all (only `PWRB`,
`XHCI`, `AWAC`, and others), and `dmesg` logs `ACPI: button: [Firmware
Bug]: Unexpected lid state reported by firmware`. By contrast, in the same
DSDT, the keyboard device (`PS2K`) has a `_PSW` method and the touchpad's
ACPI GPIO interrupt resource is explicitly flagged `ExclusiveAndWake` --
both of those *do* have a working wake path, which is consistent with why
touching either cancels suspend while lid-open doesn't.

The lid's `Notify (LID0, 0x80)` status-change calls are dispatched through
the embedded controller's own GPE (`dmesg`: `ACPI: EC: GPE=0x6e`), which is
real wake-capable hardware -- firmware just never marks it for wake.

**Fix, adapted from prior art on different hardware**:
[linux-surface/surface-gpe](https://github.com/linux-surface/surface-gpe)
solves the identical problem (no `_PRW` on the lid) on several Microsoft
Surface models, by calling `acpi_mark_gpe_for_wake()` + `acpi_enable_gpe()`
on a model-specific GPE at driver probe, then toggling
`acpi_set_gpe_wake_mask()` around each suspend/resume cycle.
[`kernel/minibook-lid-wake/`](../kernel/minibook-lid-wake/) is the same
technique, hardcoded to GPE `0x6E` for this one unit (see `CLAUDE.md`'s
single-unit convention) instead of the DMI-matched table the Surface
driver uses across many models. No MiniBook X community project
(checked: all repos in this README's Related projects list) has
documented this problem or a fix; this is adapted from Surface-specific
prior art, not MiniBook-specific prior art.

**Validated on real hardware**: after loading the module, closing the lid
and reopening it (without touching keyboard/touchpad) resumed the system.
A follow-up soak test left the lid closed for 18 minutes 10 seconds;
`journalctl -k` shows a single unbroken `PM: suspend entry` / `PM: suspend
exit` pair spanning the whole window, with the resume landing right when
the lid was reopened -- no spurious wakes during that window.

**Caveat, only partially tested**: GPE `0x6E` is the EC's *shared* event
line -- the DSDT dispatches AC-plug, battery, thermal, and other EC events
through the same `_Qxx` handlers/GPE as the lid. It's confirmed very
active during normal (awake) operation: `/sys/firmware/acpi/interrupts/gpe6E`
was sampled twice a few seconds apart while idle and awake, and incremented
34 times in that gap. That's why the 18-minute no-spurious-wake soak test
matters, but one 18-minute run isn't proof this never happens -- longer
and overnight soak tests, and a dedicated "plug in the charger while
asleep" test, are still open despite the module now loading at every boot
(see `kernel/minibook-lid-wake/README.md`).

### 11. Intermittent DSI panel-init failure at boot; fixed by forcing one `i915` driver reprobe

**Symptom**: this unit's screen intermittently boots into a visibly
corrupted state (garbled/split panel output). A shallow sleep/wake cycle
has always cleared it by hand. `dmesg`/`journalctl -k` shows `i915
0000:00:02.0: [drm] *ERROR* DSI link not ready` at the point of failure.

**Root cause, confirmed on this unit**: `i915` is a loadable module
(`CONFIG_DRM_I915=m`) pulled into the initramfs by `mkinitcpio`'s `kms`
hook, and binds very early in boot (~0.6s after boot start, confirmed via
`journalctl -k -b -o short-precise`, well before the real root is
mounted). The DSI link fails once during that first probe. First
diagnosed 2026-09-13 on kernel `7.2.4-3-cachyos`; reconfirmed still
present on `7.2.6-1-cachyos` (2026-09-19) and again during this fix's
validation (2026-09-22). Checked against the two known upstream bug
shapes at diagnosis time (an init-order bug fixed in 6.8.4; a clock-lane
regression with a community `icl_dsi.c` backport at
[vmunoz82/chuwifix](https://github.com/vmunoz82/chuwifix)) -- this exact
error message on this exact kernel matches neither; not filed upstream.
The manual sleep/wake recovery works because it forces the panel-enable
sequence to run again (observed: a full GuC/HuC firmware reload and
re-bind, `mei_hdcp`/`mei_pxp`/`snd_hda_intel` all rebinding to `i915`,
~20s after the failure), consistent with the GPU getting reset and
reinitialized correctly the second time.

**Fix**: [`kernel/minibook-dsi-reinit/`](../kernel/minibook-dsi-reinit/),
an out-of-tree module that registers a PCI bus notifier for `i915`'s bind
event (`8086:a7a9` at `0000:00:02.0`), plus an already-bound-at-init
fallback check for the case where `i915` wins the load-order race before
the notifier is live. Whichever path fires, ~1.5s later the module
performs the kernel-space equivalent of the sysfs `unbind`/`bind` dance
(`device_release_driver()` + `device_attach()`) -- the same exported
functions the sysfs files use internally, and the same underlying
recovery a manual sleep/wake already triggers, just forced automatically
within about a second of boot instead of relying on the user noticing
corruption and intervening by hand. A one-shot guard (an atomic
compare-and-swap) prevents the module's own forced rebind from
re-triggering itself. Gated behind an `active` module parameter (default
off) so the trigger logic could be validated via logging alone before the
real action was ever enabled; see
[`docs/superpowers/specs/2026-09-19-dsi-reinit-fix-design.md`](superpowers/specs/2026-09-19-dsi-reinit-fix-design.md)
for the full design rationale.

**A note on how this was validated**: an initial attempt to validate the
mechanism by manually unbinding `i915` via sysfs against the live,
in-use GNOME desktop session caused an immediate kernel crash requiring a
hard reboot -- confirmed via `journalctl -k -b -1` as a known,
currently-unfixed upstream DRM/i915 bug (`intel_mode_config_cleanup`
running before all DRM file descriptors held by an attached compositor
are closed, corrupting the framebuffer list: see
[this LKML thread](https://lkml.iu.edu/hypermail/linux/kernel/1912.2/04679.html)
and the in-flight "drm/i915: Eliminate FB usage..." patch series), not a
defect in this module. This is unrelated to the DSI bug itself, but it
means the module's real action must only ever be validated at real early
boot (before any compositor attaches), never by manually invoking it
against a live desktop session -- all validation below was done that way.

**Validated on real hardware**: after embedding the module in the
initramfs with `active=1`, three consecutive full cold boots all showed
the same clean pattern. Representative boot (`journalctl -k -b -o
short-precise`):

```
15:15:16.045  minibook_dsi_reinit: loaded (active=1)
15:15:16.819  i915 0000:00:02.0: [drm] Found alderlake_p/raptorlake_u ...
15:15:16.906  minibook_dsi_reinit: scheduling DSI reinit in 1500 ms (live bus notifier)
15:15:18.182  i915 0000:00:02.0: [drm] *ERROR* DSI link not ready
15:15:18.412  i915 0000:00:02.0: forcing driver reprobe to clear DSI init race
15:15:18.963  i915 0000:00:02.0: [drm] Found alderlake_p/raptorlake_u ...
15:15:19.040  minibook_dsi_reinit: reinit already scheduled/fired this load, ignoring trigger (live bus notifier)
15:15:19.040  i915 0000:00:02.0: reprobe complete
```

The real bug reproduced on all three boots (not merely a dry run), and
the forced reprobe cleared it automatically each time, roughly
250-350ms after the trigger fired -- well under the ~20s the manual
sleep/wake recovery takes. `device_attach` returned success (no
`device_attach failed` or "no driver claimed" log lines) on all three,
and the module's own self-triggered rebind (the second `Found` line
above) was correctly ignored by the one-shot guard rather than
re-scheduling. The user directly observed all three boots: normal
`plymouth` boot animation, no visible screen corruption at any point --
notably `plymouth` was actively rendering during the reprobe window on
the representative boot above and the reprobe still completed cleanly,
which is evidence against the reprobe itself being unsafe with an active
DRM client, in contrast to the live-desktop crash described above. A
preceding dry-run (`active=0`) boot also showed the same bug occurring
(`DSI link not ready` with no forced reprobe, since dry-run only logs),
confirming the bug's continued presence on this kernel independent of
the fix.

**Caveat, only partially tested**: three cold boots is enough to trust
the mechanism per this repo's editorial standard, but not enough to rule
out a rarer failure mode; longer-term/overnight multi-boot soak testing
is still open. The bug's underlying trigger condition (why the DSI link
fails on the first probe at all) remains unknown -- this fix treats the
symptom the same way the manual sleep/wake workaround always has, not
the root timing/hardware cause.
