#!/bin/sh
# Live-tests the vendor firmware's tablet-mode ACPI method (LTSM) via
# acpi_call, and checks whether it produces any observable Linux event on
# every plausible surface: the specific input device intel_hid creates, the
# kernel log, raw ACPI GPE interrupt counters (independent of any driver),
# and the actual D-Bus service (net.hadess.SensorProxy) GNOME consumes.
#
# *** WARNING ***
# Calling LTSM in tablet mode disables BOTH the physical keyboard and the
# touchpad immediately, via a single EC register write (the register is
# named KBCD, but empirically it cuts the touchpad too), so there is no
# touchpad-based fallback (e.g. clicking an on-screen keyboard) if something
# goes wrong. This script queues an automatic revert BEFORE ever entering
# tablet mode, specifically so nothing can get stuck disabled, but if you
# adapt this script, keep that ordering. If input is somehow still
# unresponsive well after $hold_seconds, plug in an external USB/Bluetooth
# keyboard or mouse, or fall back to a hard power-button reboot.
#
# Requires:
#   - the `acpi_call` kernel module, loaded and with /proc/acpi/call present
#       Arch/CachyOS: sudo pacman -S acpi_call-dkms && sudo modprobe acpi_call
#   - `evtest` (Arch/CachyOS: sudo pacman -S evtest)
#   - `gdbus` (part of glib2, present on any GNOME system) for the
#     SensorProxy check; skipped with a warning if unavailable
#
# Usage: test-ltsm-switch.sh ['\_SB.PC00.I2C1.ACMG.LTSM'] [hold_seconds]
#
# The ACPI path defaults to the U300 unit this was tested on. Confirm your
# own with dump-dsdt.sh before assuming it transfers -- device names and
# I2C bus numbers vary across board revisions.

set -eu

ltsm_path="${1:-\\_SB.PC00.I2C1.ACMG.LTSM}"
hold_seconds="${2:-5}"

if [ ! -e /proc/acpi/call ]; then
    echo "error: /proc/acpi/call not found -- is the acpi_call module loaded?" >&2
    echo "  try: sudo modprobe acpi_call" >&2
    exit 1
fi

# Find the Intel HID event device dynamically, so we're not hardcoding an
# event number that will differ on other machines/boots.
hid_devnode=$(awk '
    /^N: Name="Intel HID events"/ { found=1 }
    found && /^H: Handlers=/ {
        for (i = 1; i <= NF; i++) if ($i ~ /^event/) print "/dev/input/" $i
        exit
    }
' /proc/bus/input/devices)
[ -z "$hid_devnode" ] && echo "warning: couldn't find an 'Intel HID events' input device; skipping event capture" >&2

have_gdbus=0
command -v gdbus >/dev/null 2>&1 && have_gdbus=1
[ "$have_gdbus" = 0 ] && echo "warning: gdbus not found; skipping SensorProxy check" >&2

print_enabled_gpes() {
    for f in /sys/firmware/acpi/interrupts/gpe*; do
        grep -q "enabled" "$f" 2>/dev/null && echo "  $(basename "$f"): $(cat "$f")"
    done
}

sensor_proxy_props() {
    [ "$have_gdbus" = 1 ] && gdbus call --system --dest net.hadess.SensorProxy \
        --object-path /net/hadess/SensorProxy \
        --method org.freedesktop.DBus.Properties.GetAll net.hadess.SensorProxy 2>&1
}

echo "== Baseline: input devices, GPE counters, SensorProxy properties =="
cat /proc/bus/input/devices > /tmp/idev_before.txt
echo "-- enabled GPEs --"
print_enabled_gpes
echo "-- net.hadess.SensorProxy properties --"
sensor_proxy_props

echo "== Refreshing sudo credentials (avoids the backgrounded revert blocking on a password) =="
sudo -v

echo "== Queueing automatic revert to laptop mode in ${hold_seconds}s, before touching tablet mode =="
sudo -b sh -c "sleep $hold_seconds; echo '$ltsm_path 0x0' > /proc/acpi/call"

if [ -n "$hid_devnode" ]; then
    echo "== Capturing $hid_devnode for ${hold_seconds}s =="
    # Only evtest needs root; the redirect is deliberately done by the
    # unprivileged shell so the log stays user-owned and readable.
    # shellcheck disable=SC2024
    sudo timeout "$hold_seconds" evtest "$hid_devnode" > /tmp/evtest_capture.log 2>&1 &
    sleep 0.3
fi

echo "== Calling LTSM 0x1 (entering tablet mode -- keyboard/touchpad will disable now) =="
echo "$ltsm_path 0x1" | sudo tee /proc/acpi/call
echo "ACPI call return value:"
sudo cat /proc/acpi/call
echo

echo "== SensorProxy properties immediately after (still in tablet mode) =="
sensor_proxy_props

echo "== Waiting for the auto-revert to complete =="
sleep "$((hold_seconds + 2))"

if [ -n "$hid_devnode" ]; then
    echo "== Events captured on $hid_devnode during the tablet-mode window =="
    cat /tmp/evtest_capture.log
fi

echo "== Input device list: before vs. after (new devices would show here) =="
cat /proc/bus/input/devices > /tmp/idev_after.txt
diff /tmp/idev_before.txt /tmp/idev_after.txt || echo "(no differences -- no new input device appeared)"

echo "== Enabled GPEs after (compare against the baseline above) =="
print_enabled_gpes

echo "== Recent kernel log =="
sudo dmesg -T | tail -n 50
