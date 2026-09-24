use std::io::{self, Read};
use std::sync::mpsc::Sender;

/// A raw /dev/kmsg record looks like
/// "6,1234,56789,-;i915 0000:00:02.0: [drm] *ERROR* DSI link not ready"
/// -- everything before the first ';' is kernel metadata (priority,
/// sequence number, timestamp, flags), everything after is the actual
/// message. Continuation lines (key=value pairs) are not handled here;
/// this daemon only cares about the primary message line.
pub fn extract_message(raw: &str) -> &str {
    match raw.find(';') {
        Some(idx) => &raw[idx + 1..],
        None => raw,
    }
}

/// Matches only the primary DSI failure line, not the two lines that
/// accompany it in the same burst (DSI payload credits not released,
/// DSI send packet failed with -EBUSY). Those two always arrive together
/// with this one, so matching them too would count one failure three
/// times.
pub fn is_dsi_error(message: &str) -> bool {
    message.contains("DSI link not ready")
}

/// Decides, from this boot's kernel messages logged before the daemon
/// started, whether the panel was left corrupted. minibook-dsi-reinit's
/// reprobe usually clears a boot-time failure, but its own re-enable
/// fails too about 1 time in 3 (docs/findings.md, finding 11), and that
/// failure lands before this service starts. A later "reprobe complete"
/// or wake ("PM: suspend exit") re-enabled the panel, so only an error
/// after the last of those counts. Messages are oldest first.
pub fn needs_startup_repair<I, S>(messages: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut corrupted = false;
    for message in messages {
        let message = message.as_ref();
        if is_dsi_error(message) {
            corrupted = true;
        } else if message.contains("reprobe complete") || message.contains("PM: suspend exit") {
            corrupted = false;
        }
    }
    corrupted
}

/// Reads every record already in the ring buffer from `reader`, which
/// must be a /dev/kmsg fd opened with O_NONBLOCK: the read that would
/// otherwise block for a new record fails with WouldBlock (EAGAIN) instead,
/// which is how the end of the backlog is found. Returns messages only,
/// metadata stripped, oldest first.
pub fn read_backlog<R: Read>(reader: &mut R) -> io::Result<Vec<String>> {
    let mut messages = Vec::new();
    loop {
        match read_record_from(reader) {
            Ok(raw) => messages.push(extract_message(&raw).to_string()),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(messages),
            Err(e) => return Err(e),
        }
    }
}

/// Abstracts over "the next raw /dev/kmsg record" so the watch loop's
/// logic can be tested without root or a real /dev/kmsg (which this
/// unit's dmesg_restrict=1 blocks for non-root reads anyway: a non-root
/// read fails with Operation not permitted). A real read(2) on /dev/kmsg
/// returns exactly one record per call; next_record mirrors that
/// contract.
pub trait KmsgSource {
    fn next_record(&mut self) -> io::Result<String>;
}

/// Reads one record from `reader`. Retries on Interrupted (EINTR) and
/// BrokenPipe (std's mapping of EPIPE, which /dev/kmsg returns once when
/// the reader was overrun; the next read resumes at the oldest available
/// record), so neither ends the watch loop. A 0-byte read is treated as
/// EOF: real /dev/kmsg never returns 0 in blocking mode, so this only
/// stops watch from busy-spinning on a source that truly ended.
fn read_record_from<R: Read>(reader: &mut R) -> io::Result<String> {
    let mut buf = [0u8; 8192];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "kmsg source returned 0 bytes")),
            Ok(n) => return Ok(String::from_utf8_lossy(&buf[..n]).into_owned()),
            Err(e) if e.kind() == io::ErrorKind::Interrupted || e.kind() == io::ErrorKind::BrokenPipe => continue,
            Err(e) => return Err(e),
        }
    }
}

impl KmsgSource for std::fs::File {
    fn next_record(&mut self) -> io::Result<String> {
        read_record_from(self)
    }
}

/// Reads records from `source` until it errors (real /dev/kmsg never
/// naturally EOFs, so in production this only returns on a genuine I/O
/// error; tests use a fake source that errors deliberately once its
/// canned records are exhausted, to end the loop). Sends `()` on `tx`
/// for every record whose message matches `is_dsi_error`.
pub fn watch<S: KmsgSource>(source: &mut S, tx: &Sender<()>) -> io::Result<()> {
    loop {
        let raw = source.next_record()?;
        if is_dsi_error(extract_message(&raw)) {
            // The receiver may have been dropped (e.g. in a test that
            // only cares about the first N events); a send error here
            // is not this function's problem to handle, so ignore it.
            let _ = tx.send(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::mpsc;

    #[test]
    fn extract_message_strips_kmsg_metadata_prefix() {
        let raw = "6,1234,56789,-;i915 0000:00:02.0: [drm] *ERROR* DSI link not ready";
        assert_eq!(
            extract_message(raw),
            "i915 0000:00:02.0: [drm] *ERROR* DSI link not ready"
        );
    }

    #[test]
    fn extract_message_returns_whole_line_if_no_semicolon() {
        let raw = "no semicolon here";
        assert_eq!(extract_message(raw), "no semicolon here");
    }

    #[test]
    fn is_dsi_error_matches_the_real_failure_line() {
        assert!(is_dsi_error(
            "i915 0000:00:02.0: [drm] *ERROR* DSI link not ready"
        ));
    }

    #[test]
    fn is_dsi_error_does_not_match_the_other_two_burst_lines() {
        assert!(!is_dsi_error(
            "i915 0000:00:02.0: [drm] *ERROR* DSI payload credits not released"
        ));
        assert!(!is_dsi_error(
            "i915 0000:00:02.0: [drm] *ERROR* DSI send packet failed with -EBUSY"
        ));
    }

    #[test]
    fn is_dsi_error_does_not_match_unrelated_lines() {
        assert!(!is_dsi_error(
            "i915 0000:00:02.0: [drm] GT0: GuC firmware i915/adlp_guc_70.bin version 70.49.4"
        ));
    }

    struct FakeKmsg {
        records: VecDeque<String>,
    }

    impl KmsgSource for FakeKmsg {
        fn next_record(&mut self) -> io::Result<String> {
            self.records
                .pop_front()
                .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "fake exhausted"))
        }
    }

    struct FakeRead {
        results: VecDeque<io::Result<Vec<u8>>>,
    }

    impl Read for FakeRead {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            match self.results.pop_front() {
                Some(Ok(data)) => {
                    let n = data.len().min(buf.len());
                    buf[..n].copy_from_slice(&data[..n]);
                    Ok(n)
                }
                Some(Err(e)) => Err(e),
                None => Ok(0),
            }
        }
    }

    #[test]
    fn read_record_from_retries_on_interrupted() {
        let mut fake = FakeRead {
            results: VecDeque::from(vec![
                Err(io::Error::new(io::ErrorKind::Interrupted, "EINTR")),
                Ok(b"test data".to_vec()),
            ]),
        };
        let result = read_record_from(&mut fake);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "test data");
    }

    #[test]
    fn read_record_from_retries_on_broken_pipe() {
        let mut fake = FakeRead {
            results: VecDeque::from(vec![
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "EPIPE")),
                Ok(b"test data".to_vec()),
            ]),
        };
        let result = read_record_from(&mut fake);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "test data");
    }

    #[test]
    fn read_record_from_treats_zero_byte_read_as_eof() {
        let mut fake = FakeRead {
            results: VecDeque::from(vec![Ok(vec![])]),
        };
        let result = read_record_from(&mut fake);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn read_record_from_propagates_other_errors() {
        let mut fake = FakeRead {
            results: VecDeque::from(vec![
                Err(io::Error::new(io::ErrorKind::PermissionDenied, "access denied")),
            ]),
        };
        let result = read_record_from(&mut fake);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn watch_sends_only_on_matching_records() {
        let mut fake = FakeKmsg {
            records: VecDeque::from(vec![
                "6,1,0,-;i915 0000:00:02.0: [drm] GT0: GUC: RC enabled".to_string(),
                "6,2,0,-;i915 0000:00:02.0: [drm] *ERROR* DSI link not ready".to_string(),
                "6,3,0,-;i915 0000:00:02.0: [drm] *ERROR* DSI payload credits not released"
                    .to_string(),
                "6,4,0,-;i915 0000:00:02.0: [drm] *ERROR* DSI link not ready".to_string(),
            ]),
        };
        let (tx, rx) = mpsc::channel();
        let result = watch(&mut fake, &tx);
        assert!(result.is_err(), "watch should end when the fake source errors");
        assert_eq!(
            result.unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof,
            "watch should end with UnexpectedEof when source exhausted"
        );
        let received: Vec<()> = rx.try_iter().collect();
        assert_eq!(
            received.len(),
            2,
            "expected exactly 2 sends, one per 'DSI link not ready' record"
        );
    }

    const ERR: &str = "i915 0000:00:02.0: [drm] *ERROR* DSI link not ready";
    const REPROBED: &str = "i915 0000:00:02.0: reprobe complete";
    const WOKE: &str = "PM: suspend exit";

    #[test]
    fn startup_repair_not_needed_on_a_clean_boot() {
        assert!(!needs_startup_repair(["i915 0000:00:02.0: [drm] GT0: GUC: RC enabled"]));
    }

    #[test]
    fn startup_repair_not_needed_when_the_reprobe_followed_the_error() {
        // The usual boot: first probe fails, minibook-dsi-reinit's
        // reprobe clears it.
        assert!(!needs_startup_repair([ERR, REPROBED]));
    }

    #[test]
    fn startup_repair_needed_when_the_reprobe_itself_failed() {
        // Seen on 3 boots: an error after "reprobe complete".
        assert!(needs_startup_repair([ERR, REPROBED, ERR]));
        assert!(needs_startup_repair([REPROBED, ERR]));
    }

    #[test]
    fn startup_repair_needed_without_the_module() {
        assert!(needs_startup_repair([ERR]));
    }

    #[test]
    fn startup_repair_not_needed_after_a_wake_re_enabled_the_panel() {
        assert!(!needs_startup_repair([REPROBED, ERR, WOKE]));
        assert!(needs_startup_repair([REPROBED, ERR, WOKE, ERR]));
    }

    #[test]
    fn read_backlog_stops_at_would_block_and_keeps_messages_only() {
        let mut fake = FakeRead {
            results: VecDeque::from(vec![
                Ok(b"3,1,100,-;first".to_vec()),
                Err(io::Error::new(io::ErrorKind::Interrupted, "EINTR")),
                Ok(b"3,2,200,-;second".to_vec()),
                Err(io::Error::new(io::ErrorKind::WouldBlock, "EAGAIN")),
            ]),
        };
        assert_eq!(read_backlog(&mut fake).unwrap(), vec!["first", "second"]);
    }

    #[test]
    fn read_backlog_propagates_other_errors() {
        let mut fake = FakeRead {
            results: VecDeque::from(vec![Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "access denied",
            ))]),
        };
        assert!(read_backlog(&mut fake).is_err());
    }

    #[test]
    fn watch_sends_nothing_when_no_record_matches() {
        let mut fake = FakeKmsg {
            records: VecDeque::from(vec![
                "6,1,0,-;i915 0000:00:02.0: [drm] GT0: GUC: RC enabled".to_string(),
            ]),
        };
        let (tx, rx) = mpsc::channel();
        let _ = watch(&mut fake, &tx);
        assert_eq!(rx.try_iter().count(), 0);
    }
}
