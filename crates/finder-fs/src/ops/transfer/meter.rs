use std::time::{Duration, Instant};

const SPEED_SECS: f64 = 3.0;
const ETA_SECS: f64 = 5.0;
const SETTLE: Duration = Duration::from_secs(3);
const ITEMS_TO_TRUST: u64 = 16;

#[derive(Default)]
pub(super) struct Meter {
    last: Option<(Instant, u64)>,
    moving: Duration,
    bytes_per_sec: f64,
    eta: Option<f64>,
}

pub(super) struct Left {
    pub bytes: u64,
    pub items: u64,
    pub items_done: u64,
    pub counting: bool,
}

impl Meter {
    pub fn sample(&mut self, now: Instant, bytes: u64, paused: bool, left: &Left) {
        let Some((then, before)) = self.last.replace((now, bytes)) else {
            return;
        };
        let dt = now.duration_since(then);
        if paused || dt.is_zero() || before == 0 {
            return;
        }
        let secs = dt.as_secs_f64();
        let instant = bytes.saturating_sub(before) as f64 / secs;
        self.bytes_per_sec = match self.moving.is_zero() {
            true => instant,
            false => blend(self.bytes_per_sec, instant, secs, SPEED_SECS),
        };
        self.moving += dt;
        self.eta = self.estimate(left).map(|raw| match self.eta {
            Some(shown) => blend((shown - secs).max(0.0), raw, secs, ETA_SECS),
            None => raw,
        });
    }

    pub fn bytes_per_sec(&self) -> f64 {
        self.bytes_per_sec
    }

    pub fn eta(&self) -> Option<Duration> {
        self.eta.map(Duration::from_secs_f64)
    }

    fn estimate(&self, left: &Left) -> Option<f64> {
        if left.counting || self.moving < SETTLE || self.bytes_per_sec <= 0.0 {
            return None;
        }
        let by_bytes = left.bytes as f64 / self.bytes_per_sec;
        let by_items = match left.items_done >= ITEMS_TO_TRUST {
            true => left.items as f64 * self.moving.as_secs_f64() / left.items_done as f64,
            false => 0.0,
        };
        Some(by_bytes.max(by_items))
    }
}

fn blend(old: f64, new: f64, secs: f64, over: f64) -> f64 {
    old + (new - old) * (1.0 - (-secs / over).exp())
}
