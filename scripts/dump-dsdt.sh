#!/bin/sh
# Dumps and disassembles the live DSDT ACPI table.
#
# Use this to locate your own unit's tablet-mode ACPI method (search the
# resulting .dsl for "LTSM") and confirm whether it matches the ACMG/I2C1+I2C3
# layout documented in the README, or differs like every other board
# revision we've seen so far.
#
# Requires the `acpica` package (provides `acpidump` and `iasl`).
#   Arch/CachyOS: sudo pacman -S acpica
#
# Usage: dump-dsdt.sh [output-directory]   (default: current directory)

set -eu

outdir="${1:-.}"
mkdir -p "$outdir"
cd "$outdir"

echo "Dumping DSDT table..."
sudo acpidump -b -n DSDT
sudo chmod 644 dsdt.dat

echo "Disassembling..."
iasl -d dsdt.dat

echo
echo "Decompiled ACPI table written to: $outdir/dsdt.dsl"
echo "Search it for 'LTSM' to find your unit's tablet-mode method path."
