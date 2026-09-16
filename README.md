# minibookd: Tablet Mode for the Chuwi MiniBook X (U300) on Linux/GNOME

`minibookd` is a small Rust daemon that adds tablet-mode support
(keyboard/touchpad disable, a `SW_TABLET_MODE` switch for GNOME, and
auto-rotation) to the **Intel Core i3-U300** variant of the Chuwi
MiniBook X convertible, under GNOME on Wayland. It reuses the vendor
firmware's own tablet-mode ACPI method via the generic `acpi_call` kernel
module, rather than a custom kernel driver or input interception. This
board revision is newer than any previously documented by the community
and its ACPI implementation differs from the N100/N150 units covered by
existing projects -- see [Findings](#findings).

**Status:** implemented and validated end-to-end on real hardware,
including the Tablet-to-Laptop screen-rotation reset.

## AI usage

This project was built in an interactive session with Claude (Anthropic's
Claude Sonnet 5, via Claude Code): Claude disassembled the DSDT, proposed
the architecture, and wrote the scripts and the daemon in
[`daemon/`](daemon/) (an agentic multi-pass implement-and-review process,
with separate implementer and reviewer passes cross-checking each other's
work). Every command that touched live hardware, and every result that
required physical observation (keyboard/touchpad response, hinge
position, screen orientation), was run and confirmed by the repo owner on
their own machine. This README and [`docs/findings.md`](docs/findings.md)
were drafted by Claude from the session's findings and edited by the repo
owner before publishing. Nothing here is AI speculation presented as fact
without a corresponding test recorded in
[`docs/findings.md`](docs/findings.md).

## How it works

1. **Second accelerometer**: a udev rule triggered off the first
   accelerometer's appearance instantiates the second via the
   `new_device` sysfs mechanism on `i2c-3`; no kernel module needed.
2. **Keyboard/touchpad disable**: `acpi_call` invoking
   `\_SB.PC00.I2C1.ACMG.LTSM(1/0)`, reusing the vendor's own EC mechanism
   -- no interception/grabbing of real input devices.
3. **Desktop signaling**: `minibookd` computes hinge angle from both
   accelerometers (with hysteresis and jerk-filtering) and on a state
   change calls `acpi_call` (step 2) *and* emits a synthetic
   `SW_TABLET_MODE` via a `/dev/uinput` virtual switch device, since no
   runtime signal from firmware reaches Linux on this board (see
   [Findings](#findings)).
4. **Display-rotation reset**: on the Tablet-to-Laptop transition, also
   calls Mutter's `ApplyMonitorsConfig` D-Bus method to force the
   screen's rotation transform back to normal, working around a Mutter
   bug that otherwise leaves it stuck rotated.

Net result: no custom kernel module, no `intel-hid` patching, no evdev
interception of real keyboard/touchpad input -- just a generic, packaged
ACPI-calling module plus one small daemon owning one virtual switch
device. See [Findings](#findings) for why each of these choices was made.

## Installation

This has only been run on the exact unit and paths described in
[`docs/findings.md`](docs/findings.md). On any other unit, confirm your
own ACPI paths first with [`scripts/dump-dsdt.sh`](scripts/dump-dsdt.sh)
before assuming the commands below apply as-is (see
[Contributing](#contributing)).

**Prerequisites** (Arch/CachyOS package names; adjust for your distro):

```sh
sudo pacman -S acpica acpi_call-dkms evtest
sudo modprobe acpi_call
```

`acpi_call` also needs to be loaded at boot (e.g. an entry in
`/etc/modules-load.d/`) for the daemon to work after a reboot, since it
calls `LTSM` via `/proc/acpi/call` just like `scripts/test-ltsm-switch.sh`
does. Building the daemon itself needs a Rust toolchain (`cargo`).

This unit's screen also needs `video=DSI-1:panel_orientation=right_side_up
fbcon=rotate:1` on the kernel command line for correct orientation outside
of what the daemon itself corrects on the Tablet-to-Laptop transition;
check `/proc/cmdline` and add it via your bootloader if missing (see
[`docs/findings.md`](docs/findings.md)'s Hardware/software baseline for
why).

**1. Bring up the second accelerometer at boot:**

```sh
sudo cp udev/61-minibook-accelerometer.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules
sudo udevadm trigger
```

**2. Build and install the daemon:**

```sh
cd daemon
cargo build --release
sudo install -Dm755 target/release/minibookd /usr/local/bin/minibookd
```

**3. Install and enable the systemd service:**

```sh
sudo cp systemd/minibookd.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now minibookd
```

The unit's `ExecStopPost` runs `minibookd --revert-only`, so stopping the
service also reverts `LTSM` back to Laptop mode rather than leaving the
keyboard/touchpad disabled.

**4. Install the suspend/resume hook:**

```sh
sudo install -Dm755 systemd/system-sleep/minibookd /usr/lib/systemd/system-sleep/minibookd
```

The daemon has no suspend/resume awareness on its own; this hook sends it
`SIGUSR1` on resume so it re-reads the hinge angle and reconciles hardware
to it, in case the EC reset any state independently of the daemon across
suspend. No separate enablement step; systemd runs everything under
`/usr/lib/systemd/system-sleep/` automatically. Confirmed working on real
hardware ([`docs/findings.md`](docs/findings.md), finding 9).

**5. (Optional) Install the lid-wake module:** unrelated to `minibookd`
itself, but fixes a separate bug on this hardware where opening the lid
doesn't cancel suspend (only keyboard/touchpad input does). See
[`kernel/minibook-lid-wake/README.md`](kernel/minibook-lid-wake/) for
build and DKMS install steps, and
[`docs/findings.md`](docs/findings.md), finding 10, for why it's needed
and its validation status (one 18-minute soak test passed; longer-term
validation still open).

**6. Verify:** fold the hinge into a Tablet-range position (see the
Laptop/Tablet ranges in [`docs/findings.md`](docs/findings.md), finding 5)
and confirm the keyboard/touchpad disable and check `journalctl -u
minibookd` for the transition log line.

## Usage

**CLI flags** (`minibookd --help`):

| Flag | Effect |
|---|---|
| `--dry-run` | Log what would happen; never touches real ACPI/uinput. Recommended before running for real on a new unit or after any calibration change. |
| `--revert-only` | Call `LTSM(0)` once and exit (what `ExecStopPost` uses) |
| `--acpi-path <path>` | Override the `LTSM` ACPI method path |
| `--display-accel <path>` | Override the display accelerometer IIO device path |
| `--base-accel <path>` | Override the base accelerometer IIO device path |

Running the daemon for real disables the keyboard and touchpad while in
Tablet mode, the same as `test-ltsm-switch.sh` ([`docs/findings.md`](docs/findings.md),
finding 1); the signal handler and watchdog ([`docs/findings.md`](docs/findings.md),
finding 7) revert this on exit or on a stall, but a hard hang could still
require the same recovery path (external USB/Bluetooth input, or a hard
reboot).

## Findings

Full methodology and raw evidence for everything below, plus the U300's
ACPI differences from previously-documented units and the hardware/software
baseline it was tested against, are in [`docs/findings.md`](docs/findings.md).

1. `acpi_call` + `LTSM` reliably disables the EC keyboard and touchpad.
2. No runtime tablet-mode signal reaches Linux on this board, on any
   checked path (evdev, `dmesg`, `intel_vbtn`, raw ACPI GPE counters,
   `iio-sensor-proxy`'s D-Bus properties) -- `minibookd` synthesizes
   `SW_TABLET_MODE` itself via `/dev/uinput`.
3. The second (base) accelerometer comes up via the i2c `new_device`
   mechanism and persists across reboot via a udev rule.
4. A zero-calibration dot-product/`arccos` hinge angle matches expected
   values closely at three known desk positions.
5. Off-desk use needs a sensor Z-axis offset correction and a signed
   `atan2` angle, not raw `arccos` -- whole-body tilt and fold direction
   otherwise corrupt the reading. Calibrated from 18 real readings.
6. A base-tilt gate is needed to distinguish a closing lid resting
   normally on a surface from an actually folded-and-picked-up tablet.
7. Three real bugs were found and fixed during live daemon validation: a
   NUL-byte parsing bug, a missing `SIGINT`/`SIGTERM` handler,
   unsynchronized concurrent ACPI calls, and the `uinput` switch not
   being cleared on exit.
8. The screen staying in portrait after Tablet-to-Laptop was a Mutter bug
   (a stale rotation transform), not a sensor-calibration problem --
   worked around via Mutter's `ApplyMonitorsConfig`.
9. The daemon has no native suspend/resume awareness; a systemd-sleep
   hook sends it `SIGUSR1` to force reconciliation, confirmed working on
   real hardware.
10. Opening the lid didn't cancel suspend because the ACPI Lid device has
    no wake resource (`_PRW`/`_PSW`) in this unit's DSDT at all -- fixed by
    [`kernel/minibook-lid-wake/`](kernel/minibook-lid-wake/), a small
    out-of-tree module that force-arms the EC's GPE for wake instead,
    adapted from the Microsoft Surface line's identical fix. Confirmed
    working (lid-open resumes the system) and no spurious wakes over an
    18-minute closed-lid soak test; longer-term validation still open.

## Contributing

If you have a MiniBook X U300 (or any unit where `chassis_type` already
reads `31`), the scripts in [`scripts/`](scripts/) should reproduce the
findings in [`docs/findings.md`](docs/findings.md) directly. If your
`\_SB.PC00.I2C1.ACMG` doesn't match, start with
[`scripts/dump-dsdt.sh`](scripts/dump-dsdt.sh) and search the output for
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

**Porting to a new board revision:** run `scripts/dump-dsdt.sh` first and
confirm your unit's `LTSM` method and device path before assuming
anything in this repo applies -- the U300's ACPI differs from the
N100/N150 units this project started from in device names, I2C bus
numbers, and the signaling mechanism itself (see the "What's different
about the U300" comparison in [`docs/findings.md`](docs/findings.md) for
the specifics). Both `LTSM`'s halves (the EC keyboard-disable and any
runtime signal) should be verified independently on your unit rather than
assumed from this one, the same way they were here.

If you're submitting findings or code, see [`CLAUDE.md`](CLAUDE.md) for
this repo's conventions: every factual claim about the hardware needs a
recorded test (or a clear "untested" label), no extrapolating a finding
from one unit to others without evidence, and no em-dashes in prose.

## Related projects

Existing community approaches to tablet mode on the MiniBook X mostly
fall into userspace input interception (grabbing/swallowing keyboard
events) or a from-scratch platform driver (a custom kernel module plus
userspace daemons). `minibookd` takes a third approach -- reusing the
vendor firmware's own `LTSM` method via the generic `acpi_call` module --
but owes its starting point to this prior art:

- [rhalkyard/minibook-dual-accelerometer](https://github.com/rhalkyard/minibook-dual-accelerometer):
  originated the "reuse firmware's `LTSM` method" approach this project builds on (N100)
- [bazmonk/minibook-dual-accelerometer](https://github.com/bazmonk/minibook-dual-accelerometer): N150 fork (mount-matrix fix only)
- [greymouser/minibook-x-tools](https://github.com/greymouser/minibook-x-tools)
- [fstanis/chuwi-minibook](https://github.com/fstanis/chuwi-minibook)
- [petitstrawberry/minibook-support](https://github.com/petitstrawberry/minibook-support)
- [lschans/chuwi-tablet](https://github.com/lschans/chuwi-tablet)
- [sonnyp/linux-minibook-x](https://github.com/sonnyp/linux-minibook-x)

See [`docs/findings.md`](docs/findings.md)'s Motivation section for how
these compare.
