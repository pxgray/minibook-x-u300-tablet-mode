# Chuwi MiniBook X (U300): Tablet Mode Findings for Linux/GNOME

Hardware notes and reverse-engineering findings toward tablet-mode
support for the **Intel Core i3-U300** variant of the Chuwi MiniBook X
convertible, under GNOME on Wayland. This variant is a newer board revision
than any previously documented by the community and its ACPI implementation
differs from the N100/N150 units covered by existing projects.

**Status: research complete, daemon implementation not yet written.** This
document is the findings writeup; see [Status](#status--whats-left) for
what's left.

## AI usage

This investigation was carried out in an interactive session with Claude
(Anthropic's Claude Sonnet 5, via Claude Code):

- Claude read and interpreted the disassembled DSDT (identifying the
  `LTSM`/`ACMG`/`SPC0` mechanics, the GPIO pad-config decoding, and the I2C
  bus mapping), proposed the overall architecture, and wrote the scripts in
  [`scripts/`](scripts/).
- Commands that touched live hardware (ACPI method calls, kernel
  module loads, package installs, accelerometer instantiation) were run
  manually by the repo owner on their own machine. Results that require
  physical observation (the keyboard and touchpad actually going dead, the
  hinge held at a given angle) were confirmed by human observation.
- This README was drafted by Claude from the session's findings and
  edited by the repo owner before publishing.
- The daemon in [`daemon/`](daemon/) (~700 lines of Rust) was written by
  Claude in an agentic multi-pass implement-and-review process: separate
  implementer and reviewer passes cross-checked each other's work across
  the daemon's modules before this fix wave. Unlike the scripts above, no
  part of the daemon has been run against real hardware yet; that
  validation is still pending (see the "Not yet validated against real
  hardware" note in [Status](#status--whats-left)).
- The hinge-angle calibration (finding 5) was a heavily interactive,
  corrective process, not a clean implement-and-review pass: an earlier
  attempt at this same calibration was fully discarded mid-session after
  Claude asserted an unverified physical explanation (that the hinge had
  moved between readings) as fact, then jumped from data-gathering
  straight into live implementation without the repo owner's explicit
  sign-off on that pacing. All accelerometer readings in
  `calibration_data.json` were captured manually by the repo owner; the
  fitting approach, threshold placement, and Z-only vs. X+Z offset
  decision were each confirmed with the repo owner before being committed,
  after that correction.

Nothing in this document is AI speculation presented as fact without
a corresponding test recorded in [Empirical validation](#empirical-validation).
That said, the analysis, the scripts, and the writing all had substantial AI
involvement, so weigh the claims here accordingly.

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

This writeup investigates a third option: **reuse the vendor firmware's own
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

**A caveat on how `LTSM` was identified as the relevant method:** no Windows
installation was traced or examined on this unit. `LTSM` was found by
disassembling the DSDT and searching for the same method name rhalkyard
documented on a different (N100) unit, where it was described as what the
stock Windows driver (`mxc6655angle.dll`) calls, itself only described there
as "appears to," not confirmed by reverse-engineering the driver. That
assumption is carried over here, not independently verified. What this
writeup does establish directly, from this unit's own DSDT and from calling
the method (below): `LTSM` conditionally writes the EC's `KBCD`
keyboard-disable register based on its boolean argument, and calling it
produces exactly that keyboard/touchpad-disable behavior on real hardware.
That makes it a firmware-authored mode-switch method by its actions,
regardless of which OS or driver is actually meant to call it.

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
[`scripts/test-ltsm-switch.sh`](scripts/test-ltsm-switch.sh), which:

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
- **Both the physical keyboard and the touchpad visibly stopped responding**
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
tablet mode for free. A userspace daemon will need to synthesize a
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
[`udev/61-minibook-accelerometer.rules`](udev/61-minibook-accelerometer.rules)
triggers the same instantiation automatically off the display
accelerometer's `iio:device0` appearing (**confirmed working**: `iio:device1`
comes up on its own after a reboot, with no manual step), adapted from
[rhalkyard's `60-sensor-chuwi.rules`](https://github.com/rhalkyard/minibook-dual-accelerometer/blob/main/hack-driver/60-sensor-chuwi.rules)
but pointed at `i2c-3` instead of `i2c-0`, and deliberately without setting
`ACCEL_MOUNT_MATRIX` (not yet determined on this board, see
[Status](#status--whats-left)) or handing off to a daemon service (none
exists yet). Install it with:

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
board. [`scripts/vector_angle.py`](scripts/vector_angle.py) does this
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
[`scripts/calibration_data.json`](scripts/calibration_data.json)):

**Sensor Z-axis offset.** The ~6 m/s² DC offset noted in finding 4 isn't
just a magnitude curiosity: left uncorrected, it distorts the *direction*
computed from the raw vectors too, and the distortion grows with tilt.
[`scripts/calibrate_hinge_axis.py`](scripts/calibrate_hinge_axis.py) fits
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
[`scripts/calibrate_hinge_axis.py`](scripts/calibrate_hinge_axis.py)'s
output and `daemon/src/angle.rs`/`state.rs`'s test suites. Laptop-expected
readings cluster in `[45.1, 110.4]` degrees; Tablet-expected readings sit
outside that on both sides (as low as -144.1, as high as 149.0), a
comfortable margin given the daemon only needs a binary split, not
research-grade absolute-angle precision.

**Known open question, not yet resolved.** A smaller residual disagreement
remains between some same-hinge-fold reading pairs even after both
corrections (on the order of 10-15 degrees in the worst case), smaller
than the original 36-degree problem, but not fully explained. Several
hypotheses were investigated and ruled out empirically: the hinge
physically moving between readings (mechanically implausible on this
unit and explicitly ruled out), leg instability during capture (ruled out
by the specific stable knee-bent support position used), and an
unconscious screen-angle adjustment while repositioning for comfortable
typing (also ruled out). What actually causes it is unknown. It does not
appear to block practical tablet-mode detection (see the classification
margins above), and matches prior art's own experience: neither
rhalkyard's nor bazmonk's implementation for this hardware corrects for
sensor offset at all, both explicitly describing their result as
"impact[ing] accuracy... though not unusably so" rather than resolved.
This is flagged here as genuinely open, not swept under a "known
limitation" label; revisit if a real hardware cause is ever identified.

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

## Proposed architecture (implemented, not yet validated against real hardware)

This architecture is now implemented, in [`daemon/`](daemon/) (the Rust
daemon) and [`systemd/`](systemd/) (its service unit), but has not yet been
run against real hardware; see [Status](#status--whats-left).

1. **Second accelerometer**: udev rule triggered off the first accelerometer's
   appearance, instantiating the second via the `new_device` sysfs mechanism
   on `i2c-3`; no kernel module needed for this step.
2. **Keyboard/touchpad disable**: `acpi_call` calling
   `\_SB.PC00.I2C1.ACMG.LTSM(1/0)`, confirmed working. Reuses the vendor's
   own EC mechanism; no interception/grabbing of real input devices.
3. **Desktop signaling**: a small daemon computes hinge angle from both
   accelerometers (dot-product/`arccos`, validated above, with hysteresis and
   jerk-filtering along the lines of rhalkyard's `angle-sensor.py`), and on a
   state change both calls `acpi_call` (step 2) *and* emits a synthetic
   `SW_TABLET_MODE` via a `/dev/uinput` virtual switch device, purely for
   `iio-sensor-proxy`/GNOME Shell to consume.

Net result: no custom kernel module, no `intel-hid` patching, no evdev
interception of real keyboard/touchpad input, only a generic, packaged
ACPI-calling module plus one small daemon owning one virtual switch device.

## Status / what's left

- [x] Confirm `LTSM` ACPI path and EC keyboard-disable behavior
- [x] Confirm absence of a usable runtime tablet-mode signal on this
      generation (rules out a DMI-allowlist-only fix)
- [x] Bring up the second accelerometer and confirm its physical bus
- [x] Validate the hinge-angle algorithm against real measurements
- [x] udev rule to auto-instantiate the second accelerometer at boot
      (confirmed on real hardware: `iio:device1` appears after reboot with
      no manual step)
- [ ] Determine `ACCEL_MOUNT_MATRIX` for each sensor (needed for screen
      auto-rotation, not for hinge-angle detection)
- [x] Write the angle-sensor + `uinput` + `acpi_call` daemon (`daemon/`,
      Rust). It computes hinge angle from the two accelerometers, calls
      `acpi_call` to invoke `LTSM` on a state change, and emits a synthetic
      `SW_TABLET_MODE` via `/dev/uinput` for GNOME. **Not yet validated
      against real hardware** - run `minibookd --dry-run` and confirm
      logged transitions match manual hinge movement before trusting it
      live, per this repo's editorial standards; update this line and add
      an "Empirical validation" entry once that's done.
- [x] systemd service, packaging (`systemd/minibookd.service`)
- [x] Fix the `[0, 180]`-bounded hinge-angle formula's whole-body-tilt
      confound and fold-direction ambiguity: sensor Z-axis offset
      correction plus a calibrated, signed hinge angle
      (`angle::signed_hinge_angle`), replacing the original
      `angle::hinge_angle` in the live poll loop. See empirical validation
      finding 5. **Open question, not fully resolved:** a smaller (~10-15
      degree) residual disagreement remains between some same-hinge-fold
      reading pairs; cause unidentified, doesn't appear to block practical
      classification. Still pending real-hardware `--dry-run` validation,
      same as the rest of the daemon.

## Reproducing / contributing

If you have a MiniBook X U300 (or any unit where `chassis_type` already
reads `31`), the scripts in [`scripts/`](scripts/) should reproduce the
findings above directly. If your `\_SB.PC00.I2C1.ACMG` doesn't match, start
with [`scripts/dump-dsdt.sh`](scripts/dump-dsdt.sh) and search the output for
`LTSM` to find your unit's actual path, then pass it as an argument to
`test-ltsm-switch.sh`.

| Script | Purpose |
|---|---|
| [`scripts/dump-dsdt.sh`](scripts/dump-dsdt.sh) | Dump + disassemble the live DSDT to find your unit's ACPI paths |
| [`scripts/test-ltsm-switch.sh`](scripts/test-ltsm-switch.sh) | Safely test the `LTSM` ACPI method: keyboard/touchpad disable, event/dmesg/GPE/SensorProxy capture, with an automatic revert |
| [`scripts/add-second-accelerometer.sh`](scripts/add-second-accelerometer.sh) | Instantiate the second (base) accelerometer |
| [`udev/61-minibook-accelerometer.rules`](udev/61-minibook-accelerometer.rules) | Auto-instantiate the second accelerometer at boot (persists what the script above does manually) |
| [`scripts/vector_angle.py`](scripts/vector_angle.py) | Compute the angle between the two accelerometer vectors, to validate hinge-angle detection |
| [`scripts/calibrate_hinge_axis.py`](scripts/calibrate_hinge_axis.py) | Fit the sensor Z-axis offset and the hinge-axis/mounting-rotation calibration (`--self-test` for a synthetic check with no hardware needed) |
| [`scripts/calibration_data.json`](scripts/calibration_data.json) | The 18 real readings the current calibration is fitted from |

## Related projects

- [rhalkyard/minibook-dual-accelerometer](https://github.com/rhalkyard/minibook-dual-accelerometer):
  originated the "reuse firmware's `LTSM` method" approach this writeup builds on (N100)
- [bazmonk/minibook-dual-accelerometer](https://github.com/bazmonk/minibook-dual-accelerometer): N150 fork (mount-matrix fix only)
- [greymouser/minibook-x-tools](https://github.com/greymouser/minibook-x-tools)
- [fstanis/chuwi-minibook](https://github.com/fstanis/chuwi-minibook)
- [petitstrawberry/minibook-support](https://github.com/petitstrawberry/minibook-support)
- [lschans/chuwi-tablet](https://github.com/lschans/chuwi-tablet)
- [sonnyp/linux-minibook-x](https://github.com/sonnyp/linux-minibook-x)
