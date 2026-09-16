// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * Prototype: force-arms the Chuwi MiniBook X U300's ACPI Lid device (LID0,
 * PNP0C0D) as a suspend wake source.
 *
 * This unit's vendor DSDT gives LID0 no _PRW and no _PSW -- confirmed by
 * disassembling the live DSDT (see docs/findings.md). Lid-open therefore
 * cannot itself resume the system from suspend: there is no ACPI wake
 * resource for Linux to arm, which matches /proc/acpi/wakeup showing no
 * LID0 entry at all, and the "ACPI: button: [Firmware Bug]: Unexpected
 * lid state reported by firmware" line in dmesg.
 *
 * The DSDT's Notify (LID0, 0x80) status-change calls all live inside the
 * embedded controller's own _Qxx handlers, dispatched through the EC's
 * GPE -- and that GPE *is* real wake-capable hardware, per dmesg's
 * "ACPI: EC: GPE=0x6e" on this unit. Firmware just never marks it for
 * wake. This module marks and enables it directly instead, bypassing the
 * missing _PRW the same way the referenced prior art does.
 *
 * Modeled directly on linux-surface/surface-gpe's surface_gpe.c
 * (https://github.com/linux-surface/surface-gpe), which solves the
 * identical problem (no _PRW on the lid) on several Microsoft Surface
 * models via acpi_mark_gpe_for_wake()/acpi_enable_gpe() on a
 * model-specific GPE read from a DMI table. This repo documents exactly
 * one unit (see CLAUDE.md), so the GPE number below is hardcoded rather
 * than DMI-matched.
 *
 * STATUS: untested prototype. Not yet run against real hardware, not yet
 * a claim recorded in docs/findings.md's Empirical validation section per
 * this repo's editorial standards -- see kernel/minibook-lid-wake/README.md.
 *
 * KNOWN CAVEAT (also untested): GPE 0x6E is the EC's *shared* event line
 * -- AC-plug, battery, thermal, and other EC _Qxx handlers all notify
 * through it too, per the DSDT. Marking it for wake may make the system
 * also wake on those events, not lid-open alone.
 */

#define pr_fmt(fmt) KBUILD_MODNAME ": " fmt

#include <linux/acpi.h>
#include <linux/kernel.h>
#include <linux/module.h>
#include <linux/platform_device.h>

/*
 * From dmesg's "ACPI: EC: GPE=0x6e" and the DSDT's EC _Qxx Notify (LID0,
 * 0x80) handlers, both confirmed on this exact unit -- see
 * docs/findings.md.
 */
#define MINIBOOK_LID_WAKE_GPE 0x6E

static int minibook_lid_wake_set(bool enable)
{
	int action = enable ? ACPI_GPE_ENABLE : ACPI_GPE_DISABLE;
	acpi_status status;

	status = acpi_set_gpe_wake_mask(NULL, MINIBOOK_LID_WAKE_GPE, action);
	if (ACPI_FAILURE(status)) {
		pr_err("failed to set GPE wake mask: %s\n",
		       acpi_format_exception(status));
		return -EINVAL;
	}

	return 0;
}

static int __maybe_unused minibook_lid_wake_suspend(struct device *dev)
{
	return minibook_lid_wake_set(true);
}

static int __maybe_unused minibook_lid_wake_resume(struct device *dev)
{
	return minibook_lid_wake_set(false);
}

static SIMPLE_DEV_PM_OPS(minibook_lid_wake_pm, minibook_lid_wake_suspend,
			  minibook_lid_wake_resume);

static int minibook_lid_wake_probe(struct platform_device *pdev)
{
	acpi_status status;
	int ret;

	status = acpi_mark_gpe_for_wake(NULL, MINIBOOK_LID_WAKE_GPE);
	if (ACPI_FAILURE(status)) {
		dev_err(&pdev->dev, "failed to mark GPE for wake: %s\n",
			acpi_format_exception(status));
		return -EINVAL;
	}

	status = acpi_enable_gpe(NULL, MINIBOOK_LID_WAKE_GPE);
	if (ACPI_FAILURE(status)) {
		dev_err(&pdev->dev, "failed to enable GPE: %s\n",
			acpi_format_exception(status));
		return -EINVAL;
	}

	/* Wake mask starts disarmed; the PM ops arm it only across an
	 * actual suspend cycle, matching surface_gpe.c's behavior. */
	ret = minibook_lid_wake_set(false);
	if (ret)
		acpi_disable_gpe(NULL, MINIBOOK_LID_WAKE_GPE);

	return ret;
}

static void minibook_lid_wake_remove(struct platform_device *pdev)
{
	/* Restore default (no-wake) behavior without this module. */
	minibook_lid_wake_set(false);
	acpi_disable_gpe(NULL, MINIBOOK_LID_WAKE_GPE);
}

static struct platform_driver minibook_lid_wake_driver = {
	.probe = minibook_lid_wake_probe,
	.remove = minibook_lid_wake_remove,
	.driver = {
		.name = "minibook_lid_wake",
		.pm = &minibook_lid_wake_pm,
	},
};

static struct platform_device *minibook_lid_wake_device;

static int __init minibook_lid_wake_init(void)
{
	int status;

	status = platform_driver_register(&minibook_lid_wake_driver);
	if (status)
		return status;

	minibook_lid_wake_device = platform_device_register_simple(
		"minibook_lid_wake", PLATFORM_DEVID_NONE, NULL, 0);
	if (IS_ERR(minibook_lid_wake_device)) {
		status = PTR_ERR(minibook_lid_wake_device);
		platform_driver_unregister(&minibook_lid_wake_driver);
		return status;
	}

	return 0;
}
module_init(minibook_lid_wake_init);

static void __exit minibook_lid_wake_exit(void)
{
	platform_device_unregister(minibook_lid_wake_device);
	platform_driver_unregister(&minibook_lid_wake_driver);
}
module_exit(minibook_lid_wake_exit);

MODULE_DESCRIPTION("Prototype: force-arm the MiniBook X U300's lid as a suspend wake source");
MODULE_LICENSE("GPL");
