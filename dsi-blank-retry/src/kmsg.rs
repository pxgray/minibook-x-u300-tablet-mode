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
/// DSI send packet failed with -EBUSY) -- see this plan's Global
/// Constraints for why.
pub fn is_dsi_error(message: &str) -> bool {
    message.contains("DSI link not ready")
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
