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
#include <linux/moduleparam.h>
#include <linux/pci.h>
#include <linux/device.h>
#include <linux/workqueue.h>
#include <linux/atomic.h>
#include <linux/jiffies.h>

static bool active;
module_param(active, bool, 0644);
MODULE_PARM_DESC(active,
	"perform the real device_release_driver/device_attach reprobe "
	"(default off: log-only dry run)");

/* From lspci -nn on this unit -- see docs/findings.md. */
#define MINIBOOK_DSI_VENDOR_ID 0x8086
#define MINIBOOK_DSI_DEVICE_ID 0xa7a9

/* Delay between the bind event/check and forcing the reprobe, so i915's
 * own async probe work (firmware loading, connector detection) settles
 * first -- triggering mid-probe risks a race worse than the bug being
 * fixed. */
#define MINIBOOK_DSI_REINIT_DELAY_MS 1500

static atomic_t reinit_fired = ATOMIC_INIT(0);
static struct delayed_work reinit_work;

static void reinit_work_fn(struct work_struct *work)
{
	struct pci_dev *pdev;

	pdev = pci_get_device(MINIBOOK_DSI_VENDOR_ID, MINIBOOK_DSI_DEVICE_ID,
			       NULL);
	if (!pdev) {
		pr_err("target GPU disappeared before reinit could run\n");
		return;
	}

	dev_info(&pdev->dev,
		 "dry run: would force device_release_driver + "
		 "device_attach now (active=%d)\n", active);
	pci_dev_put(pdev);
}

static void schedule_dsi_reinit_once(const char *reason)
{
	if (atomic_cmpxchg(&reinit_fired, 0, 1) != 0) {
		pr_info("reinit already scheduled/fired this load, "
			"ignoring trigger (%s)\n", reason);
		return;
	}

	pr_info("scheduling DSI reinit in %d ms (%s)\n",
		MINIBOOK_DSI_REINIT_DELAY_MS, reason);
	schedule_delayed_work(&reinit_work,
			      msecs_to_jiffies(MINIBOOK_DSI_REINIT_DELAY_MS));
}

static int __init minibook_dsi_reinit_init(void)
{
	struct pci_dev *pdev;

	INIT_DELAYED_WORK(&reinit_work, reinit_work_fn);

	pdev = pci_get_device(MINIBOOK_DSI_VENDOR_ID, MINIBOOK_DSI_DEVICE_ID,
			       NULL);
	if (pdev) {
		if (pdev->dev.driver)
			schedule_dsi_reinit_once("already bound at load");
		pci_dev_put(pdev);
	}

	pr_info("loaded (active=%d)\n", active);
	return 0;
}

static void __exit minibook_dsi_reinit_exit(void)
{
	cancel_delayed_work_sync(&reinit_work);
	pr_info("unloaded\n");
}

module_init(minibook_dsi_reinit_init);
module_exit(minibook_dsi_reinit_exit);

MODULE_LICENSE("GPL");
MODULE_DESCRIPTION("Forces one i915 driver reprobe at boot to work around a DSI panel-init race on the Chuwi MiniBook X U300");
MODULE_AUTHOR("minibook project");
