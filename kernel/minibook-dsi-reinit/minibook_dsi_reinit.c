// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * Forces the Chuwi MiniBook X U300's i915 GPU (PCI 8086:a7a9) through a
 * full driver remove+probe cycle as early in boot as possible, to work
 * around an intermittent DSI panel-init failure ("[drm] *ERROR* DSI link
 * not ready") confirmed on this exact unit at kernel 7.2.6-1-cachyos --
 * see docs/findings.md.
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
 * STATUS: validated on real hardware -- three consecutive full cold boots
 * with active=1 all reproduced the actual DSI panel-init bug and cleared
 * it automatically, with no crashes and no visible corruption. See
 * docs/findings.md, finding 11, for the full log evidence. Still gated
 * behind the "active" module parameter (default off / dry-run) -- see
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
#include <linux/notifier.h>

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
	int ret;

	pdev = pci_get_device(MINIBOOK_DSI_VENDOR_ID, MINIBOOK_DSI_DEVICE_ID,
			       NULL);
	if (!pdev) {
		pr_err("target GPU disappeared before reinit could run\n");
		return;
	}

	if (!active) {
		dev_info(&pdev->dev,
			 "dry run: would force device_release_driver + "
			 "device_attach now (active=0)\n");
		pci_dev_put(pdev);
		return;
	}

	dev_info(&pdev->dev,
		 "forcing driver reprobe to clear DSI init race\n");
	device_release_driver(&pdev->dev);

	ret = device_attach(&pdev->dev);
	if (ret < 0) {
		dev_err(&pdev->dev,
			"device_attach failed after forced release: %d "
			"(leaving GPU unbound; manual sleep/wake or a "
			"reboot is still available as fallback)\n", ret);
	} else if (ret == 0) {
		dev_warn(&pdev->dev,
			 "device_attach() returned 0 (no synchronous match; "
			 "an async probe may still be pending)\n");
	} else {
		dev_info(&pdev->dev, "reprobe complete\n");
	}

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
	/* Deliberately the shared system_wq, not a private workqueue.
	 * device_release_driver() on a GPU is heavy and occupies a shared
	 * worker for ~600ms; a private workqueue would remove a theoretical
	 * self-flush-deadlock class, but that wasn't judged necessary given
	 * 3 clean validated boots on system_wq. Known, accepted tradeoff. */
	schedule_delayed_work(&reinit_work,
			      msecs_to_jiffies(MINIBOOK_DSI_REINIT_DELAY_MS));
}

static bool is_target_device(struct device *dev)
{
	struct pci_dev *pdev;

	if (!dev_is_pci(dev))
		return false;

	pdev = to_pci_dev(dev);
	return pdev->vendor == MINIBOOK_DSI_VENDOR_ID &&
	       pdev->device == MINIBOOK_DSI_DEVICE_ID;
}

static struct notifier_block reinit_nb;

static int reinit_bus_notify(struct notifier_block *nb, unsigned long action,
			      void *data)
{
	struct device *dev = data;

	if (action != BUS_NOTIFY_BOUND_DRIVER)
		return NOTIFY_DONE;

	if (!is_target_device(dev))
		return NOTIFY_DONE;

	schedule_dsi_reinit_once("live bus notifier");
	return NOTIFY_DONE;
}

static int __init minibook_dsi_reinit_init(void)
{
	struct pci_dev *pdev;

	INIT_DELAYED_WORK(&reinit_work, reinit_work_fn);

	reinit_nb.notifier_call = reinit_bus_notify;
	bus_register_notifier(&pci_bus_type, &reinit_nb);

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
	bus_unregister_notifier(&pci_bus_type, &reinit_nb);
	cancel_delayed_work_sync(&reinit_work);
	pr_info("unloaded\n");
}

module_init(minibook_dsi_reinit_init);
module_exit(minibook_dsi_reinit_exit);

MODULE_LICENSE("GPL");
MODULE_DESCRIPTION("Forces one i915 driver reprobe at boot to work around a DSI panel-init race on the Chuwi MiniBook X U300");
MODULE_AUTHOR("minibook project");
