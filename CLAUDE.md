# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this repository is

Hardware reverse-engineering findings and reproducible test scripts for
tablet-mode support on the Chuwi MiniBook X (Intel Core i3-U300 variant)
under Linux/GNOME. This is a findings writeup, not a working daemon yet:
see `README.md`'s "Status / what's left" section for what's implemented
versus not.

There is no build, lint, or test framework. The "commands" for this repo
are the scripts themselves, meant to be run directly on the actual
hardware they document.

## Running the scripts

All scripts live in `scripts/`:

- `dump-dsdt.sh [output-dir]`: dumps and disassembles the live DSDT (needs
  `acpica`: `sudo pacman -S acpica`). Run this first on any new or
  different unit before assuming this repo's ACPI paths apply to it.
- `test-ltsm-switch.sh ['<ACPI path>'] [hold_seconds]`: calls the vendor
  `LTSM` ACPI method via `acpi_call` and checks every plausible surface
  (evdev device, `dmesg`, ACPI GPE counters, `net.hadess.SensorProxy`
  D-Bus properties) for an observable effect. **This disables the physical
  keyboard and touchpad immediately** on the unit it's run on; it queues a
  timed auto-revert before ever entering tablet mode. Never remove or
  reorder that safety net when editing this script. Requires `acpi_call`
  loaded (`sudo pacman -S acpi_call-dkms && sudo modprobe acpi_call`) and
  `evtest`.
- `add-second-accelerometer.sh [i2c-bus]`: instantiates the second (base)
  accelerometer via the i2c `new_device` sysfs mechanism. Low-risk and
  reversible (`delete_device`).
- `vector_angle.py [display_iio_device] [base_iio_device]`: computes the
  angle between the two accelerometers' raw vectors, the core hinge-angle
  computation. Read-only, no risk.

## Architecture: how the findings layer together

1. **ACPI disassembly** (`dump-dsdt.sh`) locates the vendor's `LTSM`
   tablet-mode method and its enclosing device path. Device names and I2C
   bus numbers are confirmed to differ across MiniBook X board revisions
   (`ACMK`/`I2C0`+`I2C1` on N100/N150 vs. `ACMG`/`I2C1`+`I2C3` on this U300
   unit). Never assume a path from one unit's writeup transfers to another
   without re-running this step.
2. **`LTSM` behavior** (`test-ltsm-switch.sh`) is tested via the generic
   `acpi_call` module rather than a custom kernel driver. On this unit,
   `LTSM` does two independent things: an EC register write (`KBCD`) that
   demonstrably disables keyboard and touchpad, and a raw GPIO pad-config
   write (`SPC0`) confirmed, across every checked surface, to produce no
   observable effect on Linux. Both halves of that conclusion were reached
   empirically, not assumed. If re-testing on different hardware, verify
   both independently rather than trusting this repo's conclusion.
3. **Accelerometer bring-up** (`add-second-accelerometer.sh`) and
   **hinge-angle math** (`vector_angle.py`) are independent of the `LTSM`
   findings: the angle computation works from raw accelerometer vectors
   alone and needs no ACPI method calls.

See `README.md` for the full empirical results, the proposed (not yet
implemented) daemon architecture, and prior art from other MiniBook X
Linux projects.

## Editorial standards for this repository

This repo's credibility rests on every claim being backed by an actual
test recorded in `README.md`'s "Empirical validation" section, not by
analogy or inference presented as fact:

- Before adding a new factual claim about this hardware to `README.md`,
  verify it empirically on the actual machine first, or clearly mark it as
  untested/inherited from another project (as done for the "Windows
  driver" attribution).
- Do not extrapolate a finding from one unit to "other units of the same
  variant" without evidence; state only what was actually observed.
- No em-dashes in prose. This is a standing style preference for this
  repo, corrected once already.
- Keep the AI usage disclosure section in `README.md` accurate if the
  division of labor between AI and manual verification changes.
