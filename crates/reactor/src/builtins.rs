//! Idle/exit policy only. Native retains the `std::function` payloads and their calls;
//! the deferred queue is `deferred.rs`.
#[repr(C)]
pub struct ReactorBuiltinReply {
    pub phase: u32,
    pub action: i32,
}
/// Roles: 2 idle/future, 3 try-exit, 4 defer-exit, 5 default-exit.
/// Actions: 0 done/false, 6 invoke-task, 7 query-exit, 8 publish-stopped, 9 true; -1 invalid.
/// Flag 1 captures initial manager/task/wait presence; flag 2 is the Native exit
/// result after action 7. Native executes every action after return and stops
/// immediately after append/delete/invoke/stop, with no post-delete field read.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_reactor_builtin_step(
    role: u32,
    phase: u32,
    flags: u32,
    event_count: u64,
) -> ReactorBuiltinReply {
    let reply = |phase, action| ReactorBuiltinReply { phase, action };
    match (role, phase) {
        (2, 0) => reply(1, if flags & 1 != 0 { 6 } else { 0 }),
        (2, 1) => reply(1, 0),
        (3, 0) => reply(
            if flags & 1 != 0 { 1 } else { 2 },
            if flags & 1 != 0 { 7 } else { 0 },
        ),
        (3, 1) => reply(2, if flags & 2 != 0 { 8 } else { 0 }),
        (3, 2) => reply(2, 0),
        (4, 0) => reply(1, if flags & 1 == 0 { 9 } else { 0 }),
        (5, 0) => reply(1, if event_count == 0 { 9 } else { 0 }),
        _ => reply(phase, -1),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ReactorDeferred, swr_v1_reactor_deferred_push, swr_v1_reactor_run_defer};
    use std::cell::RefCell;
    use std::ffi::c_void;
    use std::ptr;

    #[test]
    fn builtins_positive_idle_and_exit_order() {
        assert_eq!(std::mem::size_of::<ReactorBuiltinReply>(), 8);
        assert_eq!(std::mem::offset_of!(ReactorBuiltinReply, action), 4);
        for present in 0..2 {
            assert_eq!(
                swr_v1_reactor_builtin_step(2, 0, present, 0).action,
                if present != 0 { 6 } else { 0 }
            );
            assert_eq!(
                swr_v1_reactor_builtin_step(4, 0, present, 0).action,
                if present == 0 { 9 } else { 0 }
            );
            let exit = swr_v1_reactor_builtin_step(3, 0, present, 0);
            if present == 0 {
                assert_eq!(exit.action, 0);
            } else {
                assert_eq!(exit.action, 7);
                for accepted in 0..2 {
                    assert_eq!(
                        swr_v1_reactor_builtin_step(3, exit.phase, accepted << 1, 0).action,
                        if accepted != 0 { 8 } else { 0 }
                    );
                }
            }
        }
        for count in [0, 1, u64::MAX] {
            assert_eq!(
                swr_v1_reactor_builtin_step(5, 0, 0, count).action,
                if count == 0 { 9 } else { 0 }
            );
        }
    }

    thread_local! {
        static TRACE: RefCell<Vec<i32>> = const { RefCell::new(Vec::new()) };
    }
    fn push(value: i32) {
        TRACE.with_borrow_mut(|trace| trace.push(value));
    }
    fn take() -> Vec<i32> {
        TRACE.with_borrow_mut(std::mem::take)
    }
    fn sentinel() -> *mut c_void {
        TRACE.with(|trace| ptr::from_ref(trace).cast_mut().cast())
    }

    type Task = (Box<dyn Fn(*mut c_void)>, *mut c_void);

    /// The fields event.cc's glue reads, with the former builtins gate's `if_exit`
    /// facade: it counts queries and may clear `wait_exit`.
    struct Reactor {
        defer_tasks: *mut ReactorDeferred,
        idle_task: Option<Task>,
        future_task: Option<Task>,
        timeout_msec: i32,
        running: bool,
        wait_exit: bool,
        accepted: bool,
        mutate_wait: bool,
        queries: u32,
    }
    impl Reactor {
        fn new() -> Self {
            Self {
                defer_tasks: ptr::null_mut(),
                idle_task: None,
                future_task: None,
                timeout_msec: 17,
                running: true,
                wait_exit: false,
                accepted: true,
                mutate_wait: false,
                queries: 0,
            }
        }
        fn if_exit(&mut self) -> bool {
            self.queries += 1;
            if self.mutate_wait {
                self.wait_exit = false;
            }
            self.accepted
        }
        /// `get_timeout_msec` in `native/reactor/swoole_reactor.h`.
        fn get_timeout_msec(&self) -> i32 {
            if self.defer_tasks.is_null() {
                self.timeout_msec
            } else {
                0
            }
        }
    }

    unsafe extern "C-unwind" fn run(task: *mut c_void) {
        let task = unsafe { Box::from_raw(task.cast::<Task>()) };
        (task.0)(task.1);
    }
    unsafe extern "C" fn dispose(task: *mut c_void) {
        drop(unsafe { Box::from_raw(task.cast::<Task>()) });
    }
    /// event.cc's `Reactor::defer` (`defer_into`).
    fn defer(r: *mut Reactor, callback: impl Fn(*mut c_void) + 'static, data: *mut c_void) {
        let task: Box<Task> = Box::new((Box::new(callback), data));
        let task = Box::into_raw(task).cast();
        let slot = unsafe { &raw mut (*r).defer_tasks };
        assert_eq!(
            unsafe { swr_v1_reactor_deferred_push(slot, run, task, Some(dispose)) },
            0
        );
    }
    /// `swr_reactor_exit_predicate`.
    fn exit_predicate(role: u32, flags: u32, events: u64) -> bool {
        match swr_v1_reactor_builtin_step(role, 0, flags, events).action {
            0 => false,
            9 => true,
            other => panic!("action {other}"),
        }
    }
    fn defer_exit(r: *mut Reactor) -> bool {
        exit_predicate(4, u32::from(unsafe { !(*r).defer_tasks.is_null() }), 0)
    }
    fn defer_end(r: *mut Reactor) {
        unsafe { swr_v1_reactor_run_defer(&raw mut (*r).defer_tasks) };
    }
    /// `execute_begin_callback`: the future task, which has no Rust step.
    fn begin(r: &Reactor) {
        if let Some((callback, data)) = &r.future_task {
            callback(*data);
        }
    }
    /// The idle end callback (`swr_reactor_run_task`).
    fn idle_end(r: &Reactor) {
        let present = u32::from(r.idle_task.is_some());
        match swr_v1_reactor_builtin_step(2, 0, present, 0).action {
            0 => {}
            6 => {
                let (callback, data) = r.idle_task.as_ref().unwrap();
                callback(*data);
            }
            other => panic!("action {other}"),
        }
    }
    /// The try-exit end callback (`swr_reactor_try_exit`).
    fn try_exit(r: &mut Reactor) {
        let mut reply = swr_v1_reactor_builtin_step(3, 0, u32::from(r.wait_exit), 0);
        if reply.action == 7 {
            let accepted = if r.if_exit() { 2 } else { 0 };
            reply = swr_v1_reactor_builtin_step(3, reply.phase, accepted, 0);
        }
        match reply.action {
            0 => {}
            8 => r.running = false,
            other => panic!("action {other}"),
        }
    }

    /// The former builtins gate's deferred cases: FIFO with data, a task's own defer
    /// waits a pass, and the defer exit condition follows the queue.
    #[test]
    fn former_builtins_gate_deferred_queue() {
        let mut reactor = Reactor::new();
        let r: *mut Reactor = &raw mut reactor;
        assert!(defer_exit(r));
        defer_end(r);
        assert!(reactor.defer_tasks.is_null());
        let first = move |data| {
            assert_eq!(data, sentinel());
            assert!(unsafe { (*r).defer_tasks.is_null() });
            push(1);
        };
        defer(r, first, sentinel());
        defer(r, |_| push(2), ptr::null_mut());
        assert!(!defer_exit(r));
        defer_end(r);
        assert_eq!(take(), [1, 2]);
        assert!(reactor.defer_tasks.is_null());
        assert!(defer_exit(r));
        let nested = move |_| {
            assert!(unsafe { (*r).defer_tasks.is_null() });
            push(3);
            defer(r, |_| push(4), ptr::null_mut());
        };
        defer(r, nested, ptr::null_mut());
        defer_end(r);
        assert_eq!(take(), [3]);
        assert!(!reactor.defer_tasks.is_null());
        assert!(!defer_exit(r));
        defer_end(r);
        assert_eq!(take(), [4]);
        assert!(reactor.defer_tasks.is_null());
        assert!(defer_exit(r));
    }

    /// The former builtins gate's begin/idle, try-exit and default-exit cases.
    #[test]
    fn former_builtins_gate_tasks_and_exit() {
        for present in [false, true] {
            let mut r = Reactor::new();
            if present {
                let future = |data| {
                    assert_eq!(data, sentinel());
                    push(5);
                };
                let idle = |data| {
                    assert_eq!(data, sentinel());
                    push(6);
                };
                r.future_task = Some((Box::new(future), sentinel()));
                r.idle_task = Some((Box::new(idle), sentinel()));
            }
            begin(&r);
            idle_end(&r);
            assert_eq!(take(), if present { vec![5, 6] } else { vec![] });
        }
        for wait in [false, true] {
            for accepted in [false, true] {
                for mutation in [false, true] {
                    let mut r = Reactor::new();
                    (r.wait_exit, r.accepted, r.mutate_wait) = (wait, accepted, mutation);
                    try_exit(&mut r);
                    assert_eq!(r.queries, u32::from(wait));
                    assert_eq!(r.running, !(wait && accepted));
                    assert_eq!(r.wait_exit, wait && !mutation);
                }
            }
        }
        for events in [0, 1, u64::MAX] {
            assert_eq!(exit_predicate(5, 0, events), events == 0);
        }
    }

    /// The former builtins gate's timeout case: a pending defer zeroes the timeout.
    #[test]
    fn former_builtins_gate_timeout_follows_the_defer_queue() {
        let mut reactor = Reactor::new();
        let r: *mut Reactor = &raw mut reactor;
        reactor.timeout_msec = 73;
        assert_eq!(reactor.get_timeout_msec(), 73);
        assert!(reactor.defer_tasks.is_null());
        let task = move |_| unsafe {
            assert_eq!((*r).get_timeout_msec(), 73);
            assert!((*r).defer_tasks.is_null());
        };
        defer(r, task, ptr::null_mut());
        assert_eq!(reactor.get_timeout_msec(), 0);
        assert!(!reactor.defer_tasks.is_null());
        defer_end(r);
        assert_eq!(reactor.get_timeout_msec(), 73);
        assert!(reactor.defer_tasks.is_null());
        reactor.timeout_msec = -1;
        assert_eq!(reactor.get_timeout_msec(), -1);
    }
}
