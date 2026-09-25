use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub fn parse_raw_component(s: &str) -> Result<i64, String> {
    s.trim()
        .parse::<i64>()
        .map_err(|e| format!("invalid accelerometer raw value {s:?}: {e}"))
}

pub fn read_vector(iio_device_dir: &Path) -> io::Result<(f64, f64, f64)> {
    let x = read_component(iio_device_dir, "in_accel_x_raw")?;
    let y = read_component(iio_device_dir, "in_accel_y_raw")?;
    let z = read_component(iio_device_dir, "in_accel_z_raw")?;
    Ok((x as f64, y as f64, z as f64))
}

fn read_component(dir: &Path, filename: &str) -> io::Result<i64> {
    let contents = std::fs::read_to_string(dir.join(filename))?;
    parse_raw_component(&contents).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Keeps the three per-axis raw-value sysfs files for one accelerometer
/// open for the process's lifetime instead of opening, reading, and
/// closing them on every poll tick. A scalar sysfs attribute regenerates
/// its content on every read() starting from offset 0 (its driver-side
/// show() callback runs fresh each read, not just on open), so seeking
/// back to 0 before each read is what makes repeated reads on a held-open
/// fd observe fresh values instead of stale ones -- without the seek, a
/// second read() would just return 0 bytes, since the fd's offset is
/// already at EOF from the previous read. Used only by main.rs's hot poll
/// loop; the rarer startup/resume reads in classify_current_orientation
/// keep using the simpler read_vector above.
pub struct AccelReader {
    dir: PathBuf,
    x: File,
    y: File,
    z: File,
    buf: String,
}

impl AccelReader {
    pub fn open(iio_device_dir: &Path) -> io::Result<Self> {
        Ok(AccelReader {
            dir: iio_device_dir.to_path_buf(),
            x: File::open(iio_device_dir.join("in_accel_x_raw"))?,
            y: File::open(iio_device_dir.join("in_accel_y_raw"))?,
            z: File::open(iio_device_dir.join("in_accel_z_raw"))?,
            buf: String::new(),
        })
    }

    /// Rereads all three axes, retrying once via a fresh reopen-by-path if
    /// the first attempt fails. A held-open fd to a sysfs device whose
    /// backing kernel object was removed (e.g. an i2c delete_device/new_device
    /// re-enumeration cycle, see scripts/add-second-accelerometer.sh) returns
    /// an error permanently, unlike a plain path-based reopen -- this restores
    /// the old read_vector's per-call reopen-by-path recovery property for
    /// that one case, without paying the cost of reopening on every call.
    pub fn read(&mut self) -> io::Result<(f64, f64, f64)> {
        match self.read_once() {
            Ok(v) => Ok(v),
            Err(_) => {
                self.reopen()?;
                self.read_once()
            }
        }
    }

    fn read_once(&mut self) -> io::Result<(f64, f64, f64)> {
        let x = Self::read_one(&mut self.x, &mut self.buf)?;
        let y = Self::read_one(&mut self.y, &mut self.buf)?;
        let z = Self::read_one(&mut self.z, &mut self.buf)?;
        Ok((x as f64, y as f64, z as f64))
    }

    fn reopen(&mut self) -> io::Result<()> {
        self.x = File::open(self.dir.join("in_accel_x_raw"))?;
        self.y = File::open(self.dir.join("in_accel_y_raw"))?;
        self.z = File::open(self.dir.join("in_accel_z_raw"))?;
        Ok(())
    }

    fn read_one(file: &mut File, buf: &mut String) -> io::Result<i64> {
        file.seek(SeekFrom::Start(0))?;
        buf.clear();
        file.read_to_string(buf)?;
        parse_raw_component(buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_integer() {
        assert_eq!(parse_raw_component("450"), Ok(450));
    }

    #[test]
    fn parses_negative_integer_with_trailing_newline() {
        assert_eq!(parse_raw_component("-1632\n"), Ok(-1632));
    }

    #[test]
    fn rejects_non_numeric_input() {
        assert!(parse_raw_component("not a number").is_err());
    }

    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Creates a throwaway directory shaped like an IIO device's sysfs
    /// entry (three in_accel_*_raw files) for AccelReader tests. Uses
    /// std::env::temp_dir() plus a process-id/counter suffix instead of a
    /// tempfile crate dependency, since this repo keeps daemon/Cargo.toml
    /// free of dev-dependencies. Caller is responsible for cleanup via
    /// std::fs::remove_dir_all.
    fn make_test_device(x: i64, y: i64, z: i64) -> std::path::PathBuf {
        let n = TEST_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("minibookd-accel-test-{}-{}", std::process::id(), n));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("in_accel_x_raw"), x.to_string()).unwrap();
        std::fs::write(dir.join("in_accel_y_raw"), y.to_string()).unwrap();
        std::fs::write(dir.join("in_accel_z_raw"), z.to_string()).unwrap();
        dir
    }

    #[test]
    fn accel_reader_reads_initial_values() {
        let dir = make_test_device(1, -2, 3);
        let mut reader = AccelReader::open(&dir).unwrap();
        assert_eq!(reader.read().unwrap(), (1.0, -2.0, 3.0));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn accel_reader_rereads_updated_values_on_subsequent_call() {
        // Proves the held-open-fd + seek(0) approach actually re-reads
        // fresh content on every call rather than returning a stale
        // cached value or nothing (a second read() without seeking back
        // to 0 would return 0 bytes, since the fd's offset is already at
        // EOF from the previous read).
        let dir = make_test_device(1, 2, 3);
        let mut reader = AccelReader::open(&dir).unwrap();
        assert_eq!(reader.read().unwrap(), (1.0, 2.0, 3.0));
        std::fs::write(dir.join("in_accel_x_raw"), "42").unwrap();
        assert_eq!(reader.read().unwrap(), (42.0, 2.0, 3.0));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn accel_reader_open_fails_on_missing_device_dir() {
        let dir = std::env::temp_dir().join(format!(
            "minibookd-accel-test-nonexistent-{}",
            std::process::id()
        ));
        std::fs::remove_dir_all(&dir).ok();
        assert!(AccelReader::open(&dir).is_err());
    }

    #[test]
    fn accel_reader_read_recovers_after_one_reopen_on_error() {
        let n = TEST_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("minibookd-accel-test-{}-{}", std::process::id(), n));
        std::fs::create_dir_all(&dir).unwrap();
        // in_accel_x_raw starts out as a directory: File::open succeeds on a
        // directory on Linux, but read_to_string on it fails with EISDIR --
        // a stand-in for a device path that's temporarily unusable.
        std::fs::create_dir(dir.join("in_accel_x_raw")).unwrap();
        std::fs::write(dir.join("in_accel_y_raw"), "2").unwrap();
        std::fs::write(dir.join("in_accel_z_raw"), "3").unwrap();

        let mut reader = AccelReader::open(&dir).unwrap();

        // Replace the directory with a real file before the read happens,
        // simulating the path becoming usable again (e.g. after a device
        // re-enumeration).
        std::fs::remove_dir(dir.join("in_accel_x_raw")).unwrap();
        std::fs::write(dir.join("in_accel_x_raw"), "42").unwrap();

        assert_eq!(reader.read().unwrap(), (42.0, 2.0, 3.0));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn accel_reader_read_returns_error_when_reopen_also_fails() {
        let n = TEST_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("minibookd-accel-test-{}-{}", std::process::id(), n));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir(dir.join("in_accel_x_raw")).unwrap();
        std::fs::write(dir.join("in_accel_y_raw"), "2").unwrap();
        std::fs::write(dir.join("in_accel_z_raw"), "3").unwrap();

        let mut reader = AccelReader::open(&dir).unwrap();
        // in_accel_x_raw is left as a directory: both the original read and
        // the post-reopen retry hit the same EISDIR failure, so read() must
        // propagate an error rather than panicking or looping.
        assert!(reader.read().is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
