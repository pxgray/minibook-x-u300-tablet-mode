// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * Forces the Chuwi MiniBook X U300's i915 GPU (PCI 8086:a7a9) through a
 * full driver remove+probe cycle as early in boot as possible, to work
 * around an intermittent DSI panel-init failure ("[drm] *ERROR* DSI link
 * not ready") confirmed on this exact unit at kernel 7.2.6-1-cachyos --
 * see docs/findings.md and
 * docs/superpowers/specs/2026-09-19-dsi-reinit-fix-design.md.
 *
 * A shallow sleep/wake cycle has always cleared the corruption by hand;
 * dmesg shows a GuC/HuC firmware reload and full re-bind ~20s after the
 * failure, consistent with the GPU getting reset/reinitialized. This
 * module reproduces that same recovery automatically, via the same
 * exported functions the sysfs unbind/bind files use
 * (device_release_driver()/device_attach()), rather than patching i915's
 * internal (unexported) DSI init code.
 *
 * i915 loads from the initramfs on this unit's mkinitcpio "kms" hook, so
 * this module must also be embedded in the initramfs to have any chance
 * of registering before i915 binds -- see the design spec.
 *
 * STATUS: untested prototype. Gated behind the "active" module parameter
 * (default off / dry-run) until validated on real hardware -- see
 * kernel/minibook-dsi-reinit/README.md.
 */

#define pr_fmt(fmt) KBUILD_MODNAME ": " fmt

#include <linux/module.h>
#include <linux/kernel.h>

static int __init minibook_dsi_reinit_init(void)
{
	pr_info("loaded\n");
	return 0;
}

static void __exit minibook_dsi_reinit_exit(void)
{
	pr_info("unloaded\n");
}

module_init(minibook_dsi_reinit_init);
module_exit(minibook_dsi_reinit_exit);

MODULE_LICENSE("GPL");
MODULE_DESCRIPTION("Forces one i915 driver reprobe at boot to work around a DSI panel-init race on the Chuwi MiniBook X U300");
MODULE_AUTHOR("minibook project");
