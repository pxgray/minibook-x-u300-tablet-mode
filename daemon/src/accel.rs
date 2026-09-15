use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

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
    x: File,
    y: File,
    z: File,
    buf: String,
}

impl AccelReader {
    pub fn open(iio_device_dir: &Path) -> io::Result<Self> {
        Ok(AccelReader {
            x: File::open(iio_device_dir.join("in_accel_x_raw"))?,
            y: File::open(iio_device_dir.join("in_accel_y_raw"))?,
            z: File::open(iio_device_dir.join("in_accel_z_raw"))?,
            buf: String::new(),
        })
    }

    pub fn read(&mut self) -> io::Result<(f64, f64, f64)> {
        let x = Self::read_one(&mut self.x, &mut self.buf)?;
        let y = Self::read_one(&mut self.y, &mut self.buf)?;
        let z = Self::read_one(&mut self.z, &mut self.buf)?;
        Ok((x as f64, y as f64, z as f64))
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
        let dir = std::env::temp_dir().join(format!(
            "minibookd-accel-test-{}-{}",
            std::process::id(),
            n
        ));
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
}
