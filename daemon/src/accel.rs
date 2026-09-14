use std::io;
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
}
