//! Server pings, so a peer that went away without closing (a laptop that
//! slept, a dropped network) is dropped instead of keeping its viewer, size
//! ownership, or client until TCP gives up. Browsers answer pings themselves,
//! even in throttled background tabs.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

pub const INTERVAL: Duration = Duration::from_secs(15);
const TIMEOUT: Duration = Duration::from_secs(45);

/// When the peer last sent anything.
pub struct Keepalive {
    start: Instant,
    last_ms: AtomicU64,
}

impl Keepalive {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
            last_ms: AtomicU64::new(0),
        }
    }

    /// Call for every frame received, pongs included.
    pub fn seen(&self) {
        self.last_ms
            .store(self.start.elapsed().as_millis() as u64, Ordering::Relaxed);
    }

    pub fn expired(&self) -> bool {
        let last = Duration::from_millis(self.last_ms.load(Ordering::Relaxed));
        self.start.elapsed().saturating_sub(last) > TIMEOUT
    }

    /// Awaits `work`, which holds up reading frames (a slow command, a full
    /// PTY input queue). The peer counts as alive meanwhile, and `answer` runs
    /// each second so the tab's probes, queued behind it, are answered.
    pub async fn busy<T>(
        &self,
        work: impl std::future::Future<Output = T>,
        mut answer: impl FnMut(),
    ) -> T {
        tokio::pin!(work);
        loop {
            tokio::select! {
                result = &mut work => return result,
                () = tokio::time::sleep(Duration::from_secs(1)) => {
                    self.seen();
                    answer();
                }
            }
        }
    }
}

/// Ticks every `INTERVAL`, starting one interval from now.
pub fn ticker() -> tokio::time::Interval {
    let mut tick = tokio::time::interval_at(tokio::time::Instant::now() + INTERVAL, INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    tick
}

pub fn ping() -> axum::extract::ws::Message {
    axum::extract::ws::Message::Ping(Default::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expires_without_frames() {
        let keepalive = Keepalive {
            start: Instant::now() - TIMEOUT - Duration::from_secs(1),
            last_ms: AtomicU64::new(0),
        };
        assert!(keepalive.expired());
        keepalive.seen();
        assert!(!keepalive.expired());
    }

    #[tokio::test]
    async fn busy_work_answers_and_stays_alive() {
        let keepalive = Keepalive {
            start: Instant::now() - TIMEOUT - Duration::from_secs(1),
            last_ms: AtomicU64::new(0),
        };
        let mut answers = 0;
        let work = tokio::time::sleep(Duration::from_millis(1200));
        let result = keepalive
            .busy(
                async {
                    work.await;
                    7
                },
                || answers += 1,
            )
            .await;
        assert_eq!((result, answers), (7, 1));
        assert!(!keepalive.expired());
    }
}
