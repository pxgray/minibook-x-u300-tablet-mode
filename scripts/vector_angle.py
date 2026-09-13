#!/usr/bin/env python3
"""Prints the angle between the two accelerometer vectors.

This is the exact computation used to validate the hinge-angle algorithm in
the README's "Empirical validation" section: arccos of the normalized dot
product between the display and base accelerometer's raw (x, y, z) vectors,
deliberately with no mount-matrix / calibration applied. It exists to answer
one question -- is the raw geometry alone enough to recover the hinge angle
on this hardware? -- and it is not itself the tablet-mode daemon.

Usage:
    vector_angle.py [display_iio_device] [base_iio_device]

Defaults to iio:device0 (display) and iio:device1 (base), matching the
two-accelerometer setup from add-second-accelerometer.sh.
"""
import math
import sys
from pathlib import Path

IIO_ROOT = Path("/sys/bus/iio/devices")
Vector = tuple[int, int, int]


def read_vector(device: str) -> Vector:
    base = IIO_ROOT / device
    return tuple(
        int((base / f"in_accel_{axis}_raw").read_text())
        for axis in ("x", "y", "z")
    )


def angle_between(v1: Vector, v2: Vector) -> float:
    dot = sum(a * b for a, b in zip(v1, v2))
    mag1 = math.sqrt(sum(a * a for a in v1))
    mag2 = math.sqrt(sum(b * b for b in v2))
    cos_theta = max(-1.0, min(1.0, dot / (mag1 * mag2)))
    return math.degrees(math.acos(cos_theta))


def main() -> None:
    display_dev = sys.argv[1] if len(sys.argv) > 1 else "iio:device0"
    base_dev = sys.argv[2] if len(sys.argv) > 2 else "iio:device1"

    display_vec = read_vector(display_dev)
    base_vec = read_vector(base_dev)

    print(f"display ({display_dev}): {display_vec}")
    print(f"base    ({base_dev}):    {base_vec}")
    print(f"angle between vectors:   {angle_between(display_vec, base_vec):.1f} deg")


if __name__ == "__main__":
    main()
