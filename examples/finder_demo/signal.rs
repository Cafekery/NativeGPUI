use std::future::{Future, poll_fn};
use std::sync::Mutex;
use std::task::{Poll, Waker};

/// Rung from any thread; wakes every task awaiting it.
pub(crate) struct Signal(Mutex<(u64, Vec<Waker>)>);

impl Signal {
    pub(crate) const fn new() -> Self {
        Self(Mutex::new((0, Vec::new())))
    }

    pub(crate) fn ring(&self) {
        let mut rung = self.0.lock().unwrap();
        rung.0 += 1;
        rung.1.drain(..).for_each(Waker::wake);
    }

    pub(crate) fn count(&self) -> u64 {
        self.0.lock().unwrap().0
    }

    /// Resolves once rung after `seen`, with the count to wait past next.
    pub(crate) fn after(&self, seen: u64) -> impl Future<Output = u64> + '_ {
        poll_fn(move |cx| {
            let mut rung = self.0.lock().unwrap();
            if rung.0 != seen {
                return Poll::Ready(rung.0);
            }
            if !rung.1.iter().any(|waker| waker.will_wake(cx.waker())) {
                rung.1.push(cx.waker().clone());
            }
            Poll::Pending
        })
    }
}
