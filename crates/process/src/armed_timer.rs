//! A one-shot timer armed over pool state: its owner holds it, and dropping it
//! deletes the timer, so the state the callback reads outlives the timer.

use std::ffi::c_long;

/// Deletes timer `id` from this thread's timer if it is still there
/// (`swoole_rs_timer::timer_cancel`).
pub type Cancel = extern "C-unwind" fn(id: c_long);

/// The kill timer `ProcessPool::wait` arms over the pool, or the reload timer a
/// `ReloadTask` arms over itself. Native code arms the timer; this owns it.
#[repr(C)]
pub struct ArmedTimer {
    id: c_long,
    cancel: Cancel,
}

impl ArmedTimer {
    /// Owns timer `id`, which `cancel` deletes.
    #[must_use]
    pub fn new(id: c_long, cancel: Cancel) -> Self {
        Self { id, cancel }
    }
}

impl Drop for ArmedTimer {
    fn drop(&mut self) {
        (self.cancel)(self.id);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::cell::RefCell;

    thread_local! {
        static CANCELLED: RefCell<Vec<c_long>> = const { RefCell::new(Vec::new()) };
    }

    /// A [`Cancel`] that records the ids it deletes.
    pub(crate) extern "C-unwind" fn record(id: c_long) {
        CANCELLED.with_borrow_mut(|ids| ids.push(id));
    }

    /// The ids [`record`] deleted on this thread, since the last call.
    pub(crate) fn cancelled() -> Vec<c_long> {
        CANCELLED.take()
    }

    #[test]
    fn dropping_the_owner_deletes_the_timer() {
        let timer = Some(ArmedTimer::new(7, record));
        assert_eq!(cancelled(), []);
        drop(timer);
        assert_eq!(cancelled(), [7]);
    }
}
