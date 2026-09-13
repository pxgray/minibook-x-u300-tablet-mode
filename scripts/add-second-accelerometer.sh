#!/bin/sh
# Instantiates the second (base) accelerometer, which this board's ACPI
# tables fail to auto-enumerate -- only one of the two MXC4005 chips (the
# display one) gets brought up automatically at boot.
#
# The default bus below (i2c-3) is what `_SB.PC00.I2C3` maps to on the U300
# variant this was tested on. This is NOT the same bus number used on older
# (N100/N150) MiniBook X units documented elsewhere (which use i2c-0) --
# confirm your own mapping first if reproducing on different hardware. See
# the README's "I2C bus mapping" notes for how to trace `_SB.PC00.I2Cn` to
# its physical `/sys/bus/i2c/devices/i2c-n` adapter.
#
# This is a live, additive device instantiation -- it does not touch the
# keyboard, EC, or anything risky, and is trivially reversible (see the end
# of this script's output).
#
# Does not survive reboot yet; a udev rule to automate this on boot is not
# yet written (see README status).
#
# Usage: add-second-accelerometer.sh [i2c-bus]   (default: i2c-3)

set -eu

i2c_bus="${1:-i2c-3}"
addr=0x15

echo "Instantiating mxc4005 at $addr on $i2c_bus..."
sudo sh -c "echo mxc4005 $addr > /sys/bus/i2c/devices/$i2c_bus/new_device"
sleep 1

echo
echo "IIO devices now present:"
for dev in /sys/bus/iio/devices/iio:device*; do
    [ -e "$dev/name" ] && echo "  $dev: $(cat "$dev/name")"
done

echo
echo "To undo: echo $addr | sudo tee /sys/bus/i2c/devices/$i2c_bus/delete_device"
