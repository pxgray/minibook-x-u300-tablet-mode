use std::io;
use std::sync::Mutex;

const ACPI_CALL_PATH: &str = "/proc/acpi/call";

// /proc/acpi/call is a single shared kernel resource with a strict
// write-the-call-then-read-the-response protocol. The main poll loop, the
// watchdog thread, and the SIGINT/SIGTERM handler can all call
// set_tablet_mode independently; without this lock, two calls landing at
// once could interleave their writes and reads, corrupting which ACPI
// method invocation the EC actually sees.
static CALL_LOCK: Mutex<()> = Mutex::new(());

pub fn format_call(path: &str, enable: bool) -> String {
    format!("{path} 0x{}", if enable { 1 } else { 0 })
}

pub fn parse_response(s: &str) -> Result<u32, String> {
    // acpi_call's /proc/acpi/call read handler returns a fixed-size buffer
    // whose unused tail is a NUL byte; str::trim() only strips Unicode
    // whitespace, so that NUL survives unless stripped explicitly.
    let trimmed = s.trim_matches(|c: char| c.is_whitespace() || c == '\0');
    let hex = trimmed
        .strip_prefix("0x")
        .ok_or_else(|| format!("unexpected /proc/acpi/call response: {trimmed:?}"))?;
    u32::from_str_radix(hex, 16)
        .map_err(|e| format!("unparseable /proc/acpi/call response {trimmed:?}: {e}"))
}

pub fn set_tablet_mode(path: &str, enable: bool) -> io::Result<u32> {
    let _guard = CALL_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let call = format_call(path, enable);
    std::fs::write(ACPI_CALL_PATH, call)?;
    let response = std::fs::read_to_string(ACPI_CALL_PATH)?;
    parse_response(&response).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_enable_call() {
        assert_eq!(
            format_call("\\_SB.PC00.I2C1.ACMG.LTSM", true),
            "\\_SB.PC00.I2C1.ACMG.LTSM 0x1"
        );
    }

    #[test]
    fn formats_disable_call() {
        assert_eq!(
            format_call("\\_SB.PC00.I2C1.ACMG.LTSM", false),
            "\\_SB.PC00.I2C1.ACMG.LTSM 0x0"
        );
    }

    #[test]
    fn parses_hex_response() {
        // The literal response this unit's LTSM(1) returns, per README.md.
        assert_eq!(parse_response("0x44000200\n"), Ok(0x44000200));
    }

    #[test]
    fn rejects_unparseable_response() {
        assert!(parse_response("Error: AE_NOT_FOUND").is_err());
    }

    #[test]
    fn parses_response_with_trailing_nul() {
        // /proc/acpi/call's read handler returns a fixed-size buffer whose
        // unused tail is a NUL byte, which str::trim() does not strip
        // (observed live: LTSM(0) returned "0x40900102\0").
        assert_eq!(parse_response("0x40900102\0"), Ok(0x40900102));
    }
}
