# minibook-lid-wake

Out-of-tree kernel module that makes opening the lid cancel suspend on the
MiniBook X U300, by force-arming the embedded controller's GPE (`0x6E`) as
a wake source. The vendor DSDT never wires the ACPI Lid device to any wake
resource on its own; see
[`docs/findings.md`](../../docs/findings.md), finding 10, for the full
root-cause evidence and the real-hardware validation this is based on
(including an open caveat: only one 18-minute closed-lid soak test has
been run so far, not a long-duration one).

Modeled directly on
[linux-surface/surface-gpe](https://github.com/linux-surface/surface-gpe),
which solves the identical no-`_PRW`-on-the-lid problem on several
Microsoft Surface models. This unit only needs one hardcoded GPE number
(see `CLAUDE.md`'s single-unit convention) rather than that driver's
DMI-matched table.

## Hardware/software baseline

Built and validated only against this repo's one documented unit: kernel
`7.2.5-1-cachyos`, GPE `0x6E` (read from this unit's own `dmesg` and DSDT,
see finding 10). Confirm both independently before assuming they apply to
your unit, the same as every other finding in this repo.

This unit's kernel is built with Clang/LLVM, not GCC (`vermagic` and
`modinfo` will tell you which one yours uses). Building against a
Clang-built kernel with `gcc` fails with a string of `unrecognized
command-line option` errors from flags the kernel's build system only
passes to Clang; pass `LLVM=1` to `make`/`dkms` as below if you're in the
same situation.

## Build and load manually (for testing)

```sh
make LLVM=1          # drop LLVM=1 if your kernel is GCC-built
sudo insmod minibook_lid_wake.ko
```

```sh
sudo rmmod minibook_lid_wake    # cleanly reverts: disables the GPE and
                                 # clears its wake mask, same as never
                                 # having loaded it
```

## Install permanently via DKMS (survives kernel upgrades)

```sh
sudo mkdir -p /usr/src/minibook-lid-wake-0.1
sudo cp minibook_lid_wake.c Kbuild Makefile dkms.conf /usr/src/minibook-lid-wake-0.1/
sudo dkms add -m minibook-lid-wake -v 0.1
sudo dkms build -m minibook-lid-wake -v 0.1
sudo dkms install -m minibook-lid-wake -v 0.1
```

Then load it at every boot the same way `acpi_call` already is (see the
main [README](../../README.md)'s Prerequisites):

```sh
echo minibook_lid_wake | sudo tee /etc/modules-load.d/minibook-lid-wake.conf
```

## Status

Validated once on real hardware: lid-open resumes the system, and an
18-minute closed-lid soak test showed no spurious wakes. Not yet validated
over a long/overnight soak or against an AC-plug-while-asleep event on the
shared GPE -- see finding 10's caveat before trusting this unattended for
long periods.
