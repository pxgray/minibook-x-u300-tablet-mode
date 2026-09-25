use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

pub fn is_stale(last_heartbeat: Instant, now: Instant, timeout: Duration) -> bool {
    now.duration_since(last_heartbeat) > timeout
}

pub struct Watchdog {
    last_heartbeat: Arc<Mutex<Instant>>,
}

impl Watchdog {
    pub fn new() -> Self {
        Watchdog {
            last_heartbeat: Arc::new(Mutex::new(Instant::now())),
        }
    }

    pub fn heartbeat(&self) {
        *self.last_heartbeat.lock().unwrap() = Instant::now();
    }

    pub fn spawn_monitor(
        &self,
        timeout: Duration,
        on_stale: impl Fn() + Send + 'static,
    ) -> thread::JoinHandle<()> {
        let last_heartbeat = Arc::clone(&self.last_heartbeat);
        thread::spawn(move || loop {
            thread::sleep(timeout / 2);
            let last = *last_heartbeat.lock().unwrap();
            if is_stale(last, Instant::now(), timeout) {
                on_stale();
                return;
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_heartbeat_is_not_stale() {
        let t0 = Instant::now();
        assert!(!is_stale(
            t0,
            t0 + Duration::from_secs(1),
            Duration::from_secs(5)
        ));
    }

    #[test]
    fn heartbeat_older_than_timeout_is_stale() {
        let t0 = Instant::now();
        assert!(is_stale(
            t0,
            t0 + Duration::from_secs(6),
            Duration::from_secs(5)
        ));
    }

    #[test]
    fn heartbeat_exactly_at_timeout_is_not_yet_stale() {
        let t0 = Instant::now();
        assert!(!is_stale(
            t0,
            t0 + Duration::from_secs(5),
            Duration::from_secs(5)
        ));
    }
}
