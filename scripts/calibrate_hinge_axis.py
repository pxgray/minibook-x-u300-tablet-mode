#!/usr/bin/env python3
"""Calibrates each accelerometer's Z-axis DC offset and the physical hinge
axis / sensor-mounting rotation, so minibookd can compute a tilt-corrected,
signed hinge angle instead of the original desk-only, offset-uncorrected
arccos formula.

Background: both accelerometers on this unit have a stable, additive
Z-axis offset (~6 m/s^2), the same quirk independently documented in the
rhalkyard/minibook-dual-accelerometer project on a different unit. Left
uncorrected, this offset makes the *measured total magnitude* drift with
orientation even though real gravity never does, and distorts the
direction-only hinge-angle math on top of that. Once the offset is
subtracted, a second, separate correction (a fitted hinge axis + sensor
mounting rotation) resolves the remaining whole-body-tilt confound and the
180-degree fold-direction ambiguity in the original formula.

Usage:
    python3 calibrate_hinge_axis.py --self-test
    python3 calibrate_hinge_axis.py [data_file.json]

Requires: numpy, scipy (pip install numpy scipy, or your distro's
python-numpy / python-scipy packages).
"""
import json
import sys
from pathlib import Path

import numpy as np
from scipy.optimize import least_squares
from scipy.spatial.transform import Rotation


# --- Sensor Z-axis offset fitting ------------------------------------------


def fit_z_offset(raw_vectors):
    """Fits a single additive Z-axis offset such that |raw_i - (0,0,offset)|
    is as constant as possible across all readings (a sphere fit
    constrained to the Z axis only -- the other axes are too weakly
    sampled by real-world laptop postures to fit reliably, see README.md).
    Returns (offset_z, radius, residual_std_before, residual_std_after)."""

    def resid(params):
        oz, r = params
        adjusted = raw_vectors.copy()
        adjusted[:, 2] -= oz
        return np.linalg.norm(adjusted, axis=1) - r

    x0 = [0.0, np.linalg.norm(raw_vectors, axis=1).mean()]
    result = least_squares(resid, x0)
    oz, r = result.x
    mags_before = np.linalg.norm(raw_vectors, axis=1)
    adjusted = raw_vectors.copy()
    adjusted[:, 2] -= oz
    mags_after = np.linalg.norm(adjusted, axis=1)
    return oz, r, mags_before.std(), mags_after.std()


def leave_one_out_stability(raw_vectors):
    """Refits the Z-offset excluding each reading in turn. A tight spread
    across all exclusions is what distinguishes a real, stable physical
    constant from a fit overly dependent on any single reading."""
    offsets = []
    for i in range(len(raw_vectors)):
        subset = np.delete(raw_vectors, i, axis=0)
        oz, _, _, _ = fit_z_offset(subset)
        offsets.append(oz)
    return offsets


# --- Hinge axis / mounting rotation fitting --------------------------------


def hat_from_angles(phi, psi):
    """Unit vector from spherical angles. Guarantees unit norm by
    construction, so the optimizer never needs a norm constraint."""
    return np.array([
        np.sin(psi) * np.cos(phi),
        np.sin(psi) * np.sin(phi),
        np.cos(psi),
    ])


def predict_display(u, h, mount_rotvec, theta):
    """Predicts the display sensor's (offset-corrected, unit) reading given
    the base sensor's (offset-corrected, unit) reading u, the hinge axis h
    (unit vector, base frame), the mounting rotation (as a rotation
    vector), and the hinge angle theta (radians)."""
    hinge_rotation = Rotation.from_rotvec(theta * h)
    mount_rotation = Rotation.from_rotvec(mount_rotvec)
    return mount_rotation.apply(hinge_rotation.apply(u))


def unpack_params(params, num_groups):
    phi, psi = params[0], params[1]
    mount_rotvec = params[2:5]
    thetas = params[5:5 + num_groups]
    h = hat_from_angles(phi, psi)
    return h, mount_rotvec, thetas


def fit_hinge_axis(base_units, display_units, group_indices, num_groups):
    """readings are pre-normalized unit vectors. group_indices maps each
    reading to a shared-theta group (same-hinge-fold readings share an
    index; this is what pins the fit down without needing a protractor
    anywhere -- see the *_pair / *_triplet groups in calibration_data.json).
    Returns (h, mount_rotvec, thetas_by_group_index, scipy result)."""

    def residuals(params):
        h, mount_rotvec, thetas = unpack_params(params, num_groups)
        resid = []
        for u, w, g in zip(base_units, display_units, group_indices):
            predicted = predict_display(u, h, mount_rotvec, thetas[g])
            resid.append(w - predicted)
        return np.concatenate(resid)

    initial_theta = []
    for g in range(num_groups):
        idx = group_indices.index(g)
        u, w = base_units[idx], display_units[idx]
        initial_theta.append(np.arccos(np.clip(np.dot(u, w), -1.0, 1.0)))

    x0 = np.concatenate([
        [np.pi / 2, np.pi / 2],  # phi, psi for h ~ (0, 1, 0), matching
        # the informal observation that the hinge axis is roughly
        # base-frame-Y-aligned on this unit.
        [0.0, 0.0, 0.0],  # mount rotvec ~ identity
        initial_theta,
    ])

    result = least_squares(residuals, x0, method="lm")
    h, mount_rotvec, thetas = unpack_params(result.x, num_groups)
    return h, mount_rotvec, thetas, result


def signed_hinge_angle(base_raw, display_raw, base_offset, display_offset, h, mount_rotvec):
    """Runtime formula: the same computation daemon/src/angle.rs's
    signed_hinge_angle implements in Rust. Range (-180, 180]."""
    b = np.array(base_raw, dtype=float) - base_offset
    d = np.array(display_raw, dtype=float) - display_offset
    u = b / np.linalg.norm(b)
    w = d / np.linalg.norm(d)

    mount_rotation = Rotation.from_rotvec(mount_rotvec)
    w_ref = mount_rotation.inv().apply(w)

    u_par = np.dot(u, h) * h
    u_perp = u - u_par
    w_ref_par = np.dot(w_ref, h) * h
    w_ref_perp = w_ref - w_ref_par

    e1 = u_perp / np.linalg.norm(u_perp)
    e2 = np.cross(h, e1)
    x = np.dot(w_ref_perp, e1)
    y = np.dot(w_ref_perp, e2)
    return np.degrees(np.arctan2(y, x))


# --- Self-test (synthetic data, no hardware needed) ------------------------


def self_test():
    """Generates synthetic data from KNOWN hinge-axis/mount-rotation
    parameters, fits against it, and verifies the fit recovers those known
    parameters up to the model's inherent gauge freedom: rotating
    mount_rotvec by -delta about h while shifting every theta by +delta
    leaves every prediction (and the fit cost) unchanged, since two
    rotations about the same axis compose additively. So rather than
    checking mount_rotvec or any theta against its "true" value directly,
    this checks what actually IS well-defined: h itself (recovered
    exactly), the fit cost (near zero), and every fitted theta equalling
    its true theta plus the SAME constant offset."""
    true_h = np.array([0.1, 0.9, 0.3])
    true_h /= np.linalg.norm(true_h)
    true_mount_rotvec = np.array([0.2, -0.1, 0.4])

    synthetic = [
        {"id": "s1", "u": np.array([0.8, 0.1, -0.6]), "theta": 0.3, "group": "pair_a"},
        {"id": "s2", "u": np.array([-0.3, 0.7, 0.6]), "theta": 0.3, "group": "pair_a"},
        {"id": "s3", "u": np.array([0.2, -0.9, 0.4]), "theta": 1.8, "group": None},
        {"id": "s4", "u": np.array([-0.6, 0.2, -0.8]), "theta": 2.9, "group": None},
        {"id": "s5", "u": np.array([0.5, 0.5, -0.7]), "theta": -1.2, "group": None},
    ]

    base_units, display_units, group_names, group_indices = [], [], [], []
    true_thetas_by_id = {}
    for s in synthetic:
        u = s["u"] / np.linalg.norm(s["u"])
        w = predict_display(u, true_h, true_mount_rotvec, s["theta"])
        base_units.append(u)
        display_units.append(w)
        name = s["group"] if s["group"] else f"__singleton_{s['id']}"
        if name not in group_names:
            group_names.append(name)
        group_indices.append(group_names.index(name))
        true_thetas_by_id[s["id"]] = s["theta"]

    h, mount_rotvec, thetas, result = fit_hinge_axis(
        base_units, display_units, group_indices, len(group_names)
    )

    h_error = np.linalg.norm(h - true_h)

    fitted_thetas = np.array(thetas)
    true_thetas = []
    for name in group_names:
        first_idx = group_indices.index(group_names.index(name))
        true_thetas.append(true_thetas_by_id[synthetic[first_idx]["id"]])
    true_thetas = np.array(true_thetas)
    offsets = fitted_thetas - true_thetas
    offset_spread = offsets.max() - offsets.min()

    print(f"self-test: fitted h={h}, true h={true_h}, h_error={h_error:.6f}")
    print(f"self-test: theta offsets from true (should all be ~equal, gauge is free): {offsets}")
    print(f"self-test: offset spread={offset_spread:.6f}")
    print(f"self-test: cost={result.cost:.2e}")

    ok = h_error < 1e-3 and offset_spread < 1e-3 and result.cost < 1e-10
    print("self-test: PASS" if ok else "self-test: FAIL")
    return ok


# --- Main: real calibration against calibration_data.json ------------------


def main():
    if "--self-test" in sys.argv:
        sys.exit(0 if self_test() else 1)

    data_path = Path(sys.argv[1]) if len(sys.argv) > 1 \
        else Path(__file__).parent / "calibration_data.json"
    readings = json.loads(data_path.read_text())

    base_raw = np.array([r["base"] for r in readings], dtype=float)
    display_raw = np.array([r["display"] for r in readings], dtype=float)

    base_oz, base_r, base_std_before, base_std_after = fit_z_offset(base_raw)
    disp_oz, disp_r, disp_std_before, disp_std_after = fit_z_offset(display_raw)
    base_loo = leave_one_out_stability(base_raw)
    disp_loo = leave_one_out_stability(display_raw)

    base_offset = np.array([0.0, 0.0, base_oz])
    display_offset = np.array([0.0, 0.0, disp_oz])

    base_units, display_units, group_names, group_indices = [], [], [], []
    for r in readings:
        bu = np.array(r["base"], dtype=float) - base_offset
        du = np.array(r["display"], dtype=float) - display_offset
        base_units.append(bu / np.linalg.norm(bu))
        display_units.append(du / np.linalg.norm(du))
        name = r["group"] if r["group"] else f"__singleton_{r['id']}"
        if name not in group_names:
            group_names.append(name)
        group_indices.append(group_names.index(name))

    h, mount_rotvec, thetas, result = fit_hinge_axis(
        base_units, display_units, group_indices, len(group_names)
    )
    mount_quat_xyzw = Rotation.from_rotvec(mount_rotvec).as_quat()  # scipy order: x,y,z,w
    mount_quat_wxyz = (
        mount_quat_xyzw[3], mount_quat_xyzw[0], mount_quat_xyzw[1], mount_quat_xyzw[2],
    )

    lines = []
    lines.append("=== Sensor Z-axis offset fit ===")
    lines.append(
        f"BASE_OFFSET Z = {base_oz:.2f} raw counts "
        f"(std {base_std_before:.1f} -> {base_std_after:.1f}, "
        f"leave-one-out range [{min(base_loo):.2f}, {max(base_loo):.2f}])"
    )
    lines.append(
        f"DISPLAY_OFFSET Z = {disp_oz:.2f} raw counts "
        f"(std {disp_std_before:.1f} -> {disp_std_after:.1f}, "
        f"leave-one-out range [{min(disp_loo):.2f}, {max(disp_loo):.2f}])"
    )
    lines.append("")
    lines.append("=== Hinge axis / mounting rotation fit ===")
    lines.append(f"Fit cost: {result.cost:.6e}")
    lines.append(f"HINGE_AXIS (base frame, unit vector): ({h[0]:.6f}, {h[1]:.6f}, {h[2]:.6f})")
    lines.append(
        f"MOUNT_ROTATION quaternion (w,x,y,z) for Rust: "
        f"({mount_quat_wxyz[0]:.6f}, {mount_quat_wxyz[1]:.6f}, "
        f"{mount_quat_wxyz[2]:.6f}, {mount_quat_wxyz[3]:.6f})"
    )
    lines.append("")
    lines.append("Per-reading signed angle (degrees) and Laptop/Tablet classification:")
    laptop_angles, tablet_angles = [], []
    for r in readings:
        angle = signed_hinge_angle(
            r["base"], r["display"], base_offset, display_offset, h, mount_rotvec
        )
        lines.append(
            f"  {r['id']:25s} group={str(r['group']):20s} "
            f"expect={r['expect']:8s} signed_angle={angle:8.2f}"
        )
        (laptop_angles if r["expect"] == "laptop" else tablet_angles).append(angle)
    lines.append("")
    lines.append(
        f"Laptop range: [{min(laptop_angles):.2f}, {max(laptop_angles):.2f}]"
    )
    lines.append(f"Tablet values: {sorted(tablet_angles)}")

    output = "\n".join(lines)
    print(output)

    result_path = Path(__file__).parent / "calibration_result.txt"
    result_path.write_text(output + "\n")
    print(f"\nWrote {result_path}")


if __name__ == "__main__":
    main()
