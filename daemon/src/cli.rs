use std::path::PathBuf;

pub struct Cli {
    pub dry_run: bool,
    pub revert_only: bool,
    pub acpi_path: String,
    pub display_accel: PathBuf,
    pub base_accel: PathBuf,
}

const DEFAULT_ACPI_PATH: &str = "\\_SB.PC00.I2C1.ACMG.LTSM";
const DEFAULT_DISPLAY_ACCEL: &str = "/sys/bus/iio/devices/iio:device0";
const DEFAULT_BASE_ACCEL: &str = "/sys/bus/iio/devices/iio:device1";

pub fn parse(args: &[String]) -> Result<Cli, String> {
    let mut dry_run = false;
    let mut revert_only = false;
    let mut acpi_path = DEFAULT_ACPI_PATH.to_string();
    let mut display_accel = PathBuf::from(DEFAULT_DISPLAY_ACCEL);
    let mut base_accel = PathBuf::from(DEFAULT_BASE_ACCEL);

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--dry-run" => dry_run = true,
            "--revert-only" => revert_only = true,
            "--acpi-path" => {
                i += 1;
                acpi_path = args
                    .get(i)
                    .ok_or("--acpi-path requires a value")?
                    .clone();
            }
            "--display-accel" => {
                i += 1;
                display_accel = PathBuf::from(
                    args.get(i).ok_or("--display-accel requires a value")?,
                );
            }
            "--base-accel" => {
                i += 1;
                base_accel = PathBuf::from(args.get(i).ok_or("--base-accel requires a value")?);
            }
            other => return Err(format!("unknown flag: {other}")),
        }
        i += 1;
    }

    if dry_run && revert_only {
        return Err("--dry-run and --revert-only are mutually exclusive".to_string());
    }

    Ok(Cli {
        dry_run,
        revert_only,
        acpi_path,
        display_accel,
        base_accel,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn defaults_with_no_flags() {
        let cli = parse(&args(&[])).unwrap();
        assert!(!cli.dry_run);
        assert!(!cli.revert_only);
        assert_eq!(cli.acpi_path, DEFAULT_ACPI_PATH);
        assert_eq!(cli.display_accel, PathBuf::from(DEFAULT_DISPLAY_ACCEL));
        assert_eq!(cli.base_accel, PathBuf::from(DEFAULT_BASE_ACCEL));
    }

    #[test]
    fn dry_run_flag() {
        let cli = parse(&args(&["--dry-run"])).unwrap();
        assert!(cli.dry_run);
    }

    #[test]
    fn revert_only_flag() {
        let cli = parse(&args(&["--revert-only"])).unwrap();
        assert!(cli.revert_only);
    }

    #[test]
    fn acpi_path_override() {
        let cli = parse(&args(&["--acpi-path", "\\_SB.CUSTOM.LTSM"])).unwrap();
        assert_eq!(cli.acpi_path, "\\_SB.CUSTOM.LTSM");
    }

    #[test]
    fn dry_run_and_revert_only_together_is_rejected() {
        assert!(parse(&args(&["--dry-run", "--revert-only"])).is_err());
    }

    #[test]
    fn unknown_flag_is_rejected() {
        assert!(parse(&args(&["--bogus"])).is_err());
    }

    #[test]
    fn acpi_path_missing_value_is_rejected() {
        assert!(parse(&args(&["--acpi-path"])).is_err());
    }
}
