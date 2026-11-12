//! `Server`'s callbacks: upstream's on* `std::function` members and `dispatch_func`.
//! Rust owns the slots, and `Server` binds each member to its slot through
//! `detail::Callback` (`native/include/swoole_rs_server_view.h`). That view boxes
//! whatever native code assigns, so the slot holds `invoke(ctx, ...)` and the
//! `drop(ctx)` that frees the box. Rust answers the presence facts and clears slots;
//! native code still makes the calls.
use crate::ops::{
    OP_CALLBACK, OP_CLEAR_CALLBACK, OP_HAS_DISPATCH_FUNC, OP_HAS_ON_CLOSE, OP_HAS_ON_CONNECT,
};
use crate::reactor_entry::RecvData;
use crate::view::Server;
use crate::{ConnectionRecord, ServerState};
use core::ffi::{c_int, c_void};
use core::sync::atomic::AtomicPtr;
use core::sync::atomic::Ordering::Relaxed;
use swoole_rs_corebase::DataHead;
use swoole_rs_process::methods::ExitStatus;
use swoole_rs_process::task_file::Packed;
use swoole_rs_process::worker_record::{Worker, WorkerRecord};

// Slots, in `SWR_SERVER_CB_*` order. The first seven are also `OP_CALLBACK`'s
// argument and the callback its logs name.
pub const CB_CONNECT: u32 = 0;
pub const CB_CLOSE: u32 = 1;
pub const CB_BUFFER_FULL: u32 = 2;
pub const CB_BUFFER_EMPTY: u32 = 3;
pub const CB_TASK: u32 = 4;
pub const CB_RECEIVE: u32 = 5;
pub const CB_PACKET: u32 = 6;
pub const CB_START: u32 = 7;
pub const CB_BEFORE_SHUTDOWN: u32 = 8;
pub const CB_SHUTDOWN: u32 = 9;
pub const CB_MANAGER_START: u32 = 10;
pub const CB_MANAGER_STOP: u32 = 11;
pub const CB_WORKER_ERROR: u32 = 12;
pub const CB_BEFORE_RELOAD: u32 = 13;
pub const CB_AFTER_RELOAD: u32 = 14;
pub const CB_PIPE_MESSAGE: u32 = 15;
pub const CB_WORKER_START: u32 = 16;
pub const CB_WORKER_STOP: u32 = 17;
pub const CB_WORKER_EXIT: u32 = 18;
pub const CB_USER_WORKER_START: u32 = 19;
pub const CB_FINISH: u32 = 20;
pub const CB_DISPATCH_FUNC: u32 = 21;
/// `SWR_SERVER_CB_COUNT`.
pub(crate) const CB_COUNT: usize = 22;

/// The context every slot fn takes first, which a Rust fn ignores: null, or
/// what native code boxed for the fn this one replaced.
pub type Ctx = *mut c_void;
/// `void (Server *)`.
pub type ServerFn = extern "C" fn(Ctx, Server<'_>);
/// `void (Server *, DataInfo *)`.
pub type EventFn = extern "C" fn(Ctx, Server<'_>, &DataHead);
/// `int (Server *, RecvData *)`.
pub type ReceiveFn = extern "C" fn(Ctx, Server<'_>, &RecvData) -> c_int;
/// `int (Server *, EventData *)`.
pub type TaskFn = extern "C" fn(Ctx, Server<'_>, Packed<'_>) -> c_int;
/// `void (Server *, EventData *)`.
pub type MessageFn = extern "C" fn(Ctx, Server<'_>, Packed<'_>);
/// `void (Server *, Worker *)`, of a worker this thread or process runs.
pub type WorkerFn = extern "C" fn(Ctx, Server<'_>, &WorkerRecord);
/// `void (Server *, Worker *, const ExitStatus &)`.
pub type WorkerErrorFn = extern "C" fn(Ctx, Server<'_>, &WorkerRecord, &ExitStatus);
/// `void (Server *, Worker *)`, of a user worker about to start.
pub type UserWorkerFn = extern "C" fn(Ctx, Server<'_>, Worker<'_>);
/// `int (Server *, Connection *, SendData *)`.
pub type DispatchFn =
    extern "C" fn(Ctx, Server<'_>, Option<&ConnectionRecord>, Option<&RecvData>) -> c_int;

/// A Rust fn for one of a server's slots, of that slot's own signature
/// ([`Server::set_callback`]). The server calls it with a [`Ctx`] it ignores
/// and the objects of the event, lent for the call.
#[derive(Clone, Copy)]
pub enum Callback {
    Connect(EventFn),
    Close(EventFn),
    BufferFull(EventFn),
    BufferEmpty(EventFn),
    Task(TaskFn),
    Receive(ReceiveFn),
    Packet(ReceiveFn),
    Start(ServerFn),
    BeforeShutdown(ServerFn),
    Shutdown(ServerFn),
    ManagerStart(ServerFn),
    ManagerStop(ServerFn),
    WorkerError(WorkerErrorFn),
    BeforeReload(ServerFn),
    AfterReload(ServerFn),
    PipeMessage(MessageFn),
    WorkerStart(WorkerFn),
    WorkerStop(WorkerFn),
    WorkerExit(WorkerFn),
    UserWorkerStart(UserWorkerFn),
    Finish(TaskFn),
    DispatchFunc(DispatchFn),
}

impl Callback {
    /// The slot (`CB_*`) and the fn as the slot stores it.
    fn parts(self) -> (u32, *const c_void) {
        match self {
            Self::Connect(invoke) => (CB_CONNECT, invoke as *const c_void),
            Self::Close(invoke) => (CB_CLOSE, invoke as *const c_void),
            Self::BufferFull(invoke) => (CB_BUFFER_FULL, invoke as *const c_void),
            Self::BufferEmpty(invoke) => (CB_BUFFER_EMPTY, invoke as *const c_void),
            Self::Task(invoke) => (CB_TASK, invoke as *const c_void),
            Self::Receive(invoke) => (CB_RECEIVE, invoke as *const c_void),
            Self::Packet(invoke) => (CB_PACKET, invoke as *const c_void),
            Self::Start(invoke) => (CB_START, invoke as *const c_void),
            Self::BeforeShutdown(invoke) => (CB_BEFORE_SHUTDOWN, invoke as *const c_void),
            Self::Shutdown(invoke) => (CB_SHUTDOWN, invoke as *const c_void),
            Self::ManagerStart(invoke) => (CB_MANAGER_START, invoke as *const c_void),
            Self::ManagerStop(invoke) => (CB_MANAGER_STOP, invoke as *const c_void),
            Self::WorkerError(invoke) => (CB_WORKER_ERROR, invoke as *const c_void),
            Self::BeforeReload(invoke) => (CB_BEFORE_RELOAD, invoke as *const c_void),
            Self::AfterReload(invoke) => (CB_AFTER_RELOAD, invoke as *const c_void),
            Self::PipeMessage(invoke) => (CB_PIPE_MESSAGE, invoke as *const c_void),
            Self::WorkerStart(invoke) => (CB_WORKER_START, invoke as *const c_void),
            Self::WorkerStop(invoke) => (CB_WORKER_STOP, invoke as *const c_void),
            Self::WorkerExit(invoke) => (CB_WORKER_EXIT, invoke as *const c_void),
            Self::UserWorkerStart(invoke) => (CB_USER_WORKER_START, invoke as *const c_void),
            Self::Finish(invoke) => (CB_FINISH, invoke as *const c_void),
            Self::DispatchFunc(invoke) => (CB_DISPATCH_FUNC, invoke as *const c_void),
        }
    }

    /// The slot this fills (`CB_*`).
    #[must_use]
    pub fn slot(self) -> u32 {
        self.parts().0
    }
}

impl ServerCallbacks {
    /// `serv->onX = f` for the slot and fn `callback` names.
    pub(crate) fn set(&self, callback: Callback) {
        let (index, invoke) = callback.parts();
        if let Some(slot) = self.slot(index) {
            slot.set(invoke);
        }
    }
}

/// `swr_v1_server_callback`: `invoke` is a function pointer of the slot's own signature.
#[repr(C)]
pub(crate) struct CallbackSlot {
    invoke: AtomicPtr<c_void>,
    ctx: AtomicPtr<c_void>,
    drop: AtomicPtr<c_void>,
}

type DropFn = unsafe extern "C" fn(*mut c_void);

impl CallbackSlot {
    const fn new() -> Self {
        Self {
            invoke: AtomicPtr::new(core::ptr::null_mut()),
            ctx: AtomicPtr::new(core::ptr::null_mut()),
            drop: AtomicPtr::new(core::ptr::null_mut()),
        }
    }

    pub(crate) fn is_set(&self) -> bool {
        !self.invoke.load(Relaxed).is_null()
    }

    /// `serv->onX = f` for a Rust `f` of the slot's signature, which ignores
    /// its `ctx`. Only the fn changes: a context native code boxed stays with
    /// its `drop` until [`CallbackSlot::clear`], so a thread calling the slot
    /// meanwhile runs the old fn or the new one over a context that is live.
    fn set(&self, invoke: *const c_void) {
        self.invoke.store(invoke.cast_mut(), Relaxed);
    }

    /// `serv->onX(serv, data)`, for a slot of a `(Server *, EventData *)` signature
    /// whose result the caller ignores; nothing when the slot is empty.
    ///
    /// # Safety
    /// The slot holds that signature, and `serv` and `data` are what it takes.
    #[cfg(all(feature = "upstream-symbols", not(test)))]
    pub(crate) unsafe fn call(&self, serv: *mut c_void, data: *mut c_void) {
        type Invoke = unsafe extern "C-unwind" fn(*mut c_void, *mut c_void, *mut c_void);
        let invoke = self.invoke.load(Relaxed);
        // SAFETY: the slot holds null or an `R (*)(void *, Server *, EventData *)`.
        let invoke = unsafe { core::mem::transmute::<*mut c_void, Option<Invoke>>(invoke) };
        if let Some(invoke) = invoke {
            // SAFETY: per the caller, with the slot's own context.
            unsafe { invoke(self.ctx.load(Relaxed), serv, data) };
        }
    }

    /// `serv->onReceive(serv, data)`, for a slot of an `int (Server *, RecvData *)`
    /// signature; -1 when the slot is empty.
    ///
    /// # Safety
    /// The slot holds that signature, and `serv` and `data` are what it takes.
    #[cfg(all(feature = "upstream-symbols", not(test)))]
    pub(crate) unsafe fn receive(&self, serv: *mut c_void, data: *mut c_void) -> core::ffi::c_int {
        type Invoke =
            unsafe extern "C-unwind" fn(*mut c_void, *mut c_void, *mut c_void) -> core::ffi::c_int;
        let invoke = self.invoke.load(Relaxed);
        // SAFETY: the slot holds null or an `int (*)(void *, Server *, RecvData *)`.
        let invoke = unsafe { core::mem::transmute::<*mut c_void, Option<Invoke>>(invoke) };
        // SAFETY: per the caller, with the slot's own context.
        invoke.map_or(-1, |invoke| unsafe {
            invoke(self.ctx.load(Relaxed), serv, data)
        })
    }

    /// `serv->dispatch_func(serv, conn, send)`; 0 when the slot is empty.
    ///
    /// # Safety
    /// The slot is `dispatch_func`'s, and the pointers are what it takes.
    #[cfg(all(feature = "upstream-symbols", not(test)))]
    pub(crate) unsafe fn dispatch(
        &self,
        serv: *mut c_void,
        conn: *mut c_void,
        send: *mut c_void,
    ) -> core::ffi::c_int {
        type Invoke = unsafe extern "C-unwind" fn(
            *mut c_void,
            *mut c_void,
            *mut c_void,
            *mut c_void,
        ) -> core::ffi::c_int;
        let invoke = self.invoke.load(Relaxed);
        // SAFETY: the slot holds null or an `int (*)(void *, Server *, Connection *, SendData *)`.
        let invoke = unsafe { core::mem::transmute::<*mut c_void, Option<Invoke>>(invoke) };
        // SAFETY: per the caller, with the slot's own context.
        invoke.map_or(0, |invoke| unsafe {
            invoke(self.ctx.load(Relaxed), serv, conn, send)
        })
    }

    /// `serv->onWorkerError(serv, worker, status)`, a slot of a
    /// `void (Server *, Worker *, const ExitStatus &)` signature; nothing when
    /// the slot is empty. Unlike [`CallbackSlot::dispatch`], it reads no return.
    ///
    /// # Safety
    /// The slot holds that signature, and the pointers are what it takes.
    #[cfg(all(feature = "upstream-symbols", not(test)))]
    pub(crate) unsafe fn worker_error(
        &self,
        serv: *mut c_void,
        worker: *mut c_void,
        status: *mut c_void,
    ) {
        type Invoke =
            unsafe extern "C-unwind" fn(*mut c_void, *mut c_void, *mut c_void, *mut c_void);
        let invoke = self.invoke.load(Relaxed);
        // SAFETY: the slot holds null or a `void (*)(void *, Server *, Worker *, ExitStatus *)`.
        let invoke = unsafe { core::mem::transmute::<*mut c_void, Option<Invoke>>(invoke) };
        if let Some(invoke) = invoke {
            // SAFETY: per the caller, with the slot's own context.
            unsafe { invoke(self.ctx.load(Relaxed), serv, worker, status) };
        }
    }

    /// `serv->onX = nullptr`: empties the slot, then frees what it held.
    pub(crate) fn clear(&self) {
        self.invoke.store(core::ptr::null_mut(), Relaxed);
        let ctx = self.ctx.swap(core::ptr::null_mut(), Relaxed);
        let drop = self.drop.swap(core::ptr::null_mut(), Relaxed);
        if !drop.is_null() {
            // SAFETY: native code stored a `void (*)(void *) noexcept` paired with `ctx`.
            let drop = unsafe { core::mem::transmute::<*mut c_void, DropFn>(drop) };
            // SAFETY: the box is this slot's alone, and the slot no longer names it.
            unsafe { drop(ctx) };
        }
    }
}

/// Every slot of one server.
#[repr(C)]
pub(crate) struct ServerCallbacks([CallbackSlot; CB_COUNT]);

impl ServerCallbacks {
    pub(crate) const fn new() -> Self {
        Self([const { CallbackSlot::new() }; CB_COUNT])
    }

    pub(crate) fn slot(&self, index: u32) -> Option<&CallbackSlot> {
        self.0.get(index as usize)
    }
}

/// The slots of a `void (Server *)` signature.
#[cfg(all(feature = "upstream-symbols", not(test)))]
const SERVER_SLOTS: [u32; 7] = [
    CB_START,
    CB_BEFORE_SHUTDOWN,
    CB_SHUTDOWN,
    CB_MANAGER_START,
    CB_MANAGER_STOP,
    CB_BEFORE_RELOAD,
    CB_AFTER_RELOAD,
];

#[cfg(all(feature = "upstream-symbols", not(test)))]
impl ServerCallbacks {
    /// `serv->onX(serv)` for slot `index`, one of the `(Server *)` slots; nothing
    /// when the slot is empty or of another signature.
    pub(crate) fn call_server(&self, index: u32, serv: crate::view::Server<'_>) {
        type Invoke = unsafe extern "C-unwind" fn(*mut c_void, *mut c_void);
        let Some(slot) = self.slot(index).filter(|_| SERVER_SLOTS.contains(&index)) else {
            return;
        };
        let invoke = slot.invoke.load(Relaxed);
        // SAFETY: a `(Server *)` slot holds null or a `void (*)(void *, Server *)`: native
        // code assigns its typed member, and `view::Server::set_callback`'s caller one of
        // the slot's own signature.
        let invoke = unsafe { core::mem::transmute::<*mut c_void, Option<Invoke>>(invoke) };
        if let Some(invoke) = invoke {
            // SAFETY: the slot's own context and a live server.
            unsafe { invoke(slot.ctx.load(Relaxed), serv.raw()) };
        }
    }
}

impl Drop for ServerCallbacks {
    fn drop(&mut self) {
        for slot in &self.0 {
            slot.clear();
        }
    }
}

#[cfg(all(feature = "upstream-symbols", not(test)))]
impl ServerState {
    /// The first of the slots, which `Server`'s callbacks pointer heads.
    pub(crate) fn callbacks_ptr(&self) -> *mut c_void {
        self.callbacks.0.as_ptr().cast_mut().cast()
    }
}

/// Answers the callback facts from the slots, else `None`.
pub(crate) fn native_fact(state: &ServerState, op: u32, arg: u64) -> Option<u64> {
    let set = |index| {
        u64::from(
            state
                .callbacks
                .slot(index)
                .is_some_and(CallbackSlot::is_set),
        )
    };
    Some(match op {
        OP_HAS_ON_CONNECT => set(CB_CONNECT),
        OP_HAS_ON_CLOSE => set(CB_CLOSE),
        OP_HAS_DISPATCH_FUNC => set(CB_DISPATCH_FUNC),
        OP_CALLBACK => match u32::try_from(arg) {
            Ok(index @ CB_CONNECT..=CB_PACKET) => set(index),
            _ => 0,
        },
        OP_CLEAR_CALLBACK => {
            if let Ok(index @ CB_CONNECT..=CB_BUFFER_EMPTY) = u32::try_from(arg)
                && let Some(slot) = state.callbacks.slot(index)
            {
                slot.clear();
            }
            0
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::AtomicU32;

    static DROPPED: AtomicU32 = AtomicU32::new(0);

    unsafe extern "C" fn count_drop(ctx: *mut c_void) {
        assert_eq!(ctx.addr(), 0x55, "the slot's ctx");
        DROPPED.fetch_add(1, Relaxed);
    }

    unsafe extern "C" fn invoke() {}

    fn fill(slot: &CallbackSlot) {
        slot.invoke.store(invoke as *mut c_void, Relaxed);
        slot.ctx
            .store(core::ptr::without_provenance_mut(0x55), Relaxed);
        slot.drop.store(count_drop as *mut c_void, Relaxed);
    }

    #[test]
    fn a_rust_fn_replaces_the_fn_and_keeps_the_context() {
        type Invoke = extern "C" fn(*mut c_void, *mut c_void);
        static KEPT_DROPPED: AtomicU32 = AtomicU32::new(0);
        static CALLED: AtomicU32 = AtomicU32::new(0);
        unsafe extern "C" fn drop_kept(_: *mut c_void) {
            KEPT_DROPPED.fetch_add(1, Relaxed);
        }
        // Records that the slot ran it; it ignores both arguments.
        extern "C" fn start(_: Ctx, _: Server<'_>) {
            CALLED.fetch_add(1, Relaxed);
        }
        // The fn the slot holds before replacement, of `start`'s signature so
        // calling the slot is defined even if `set` failed to replace it (the
        // `CALLED` assertion then fails cleanly, not through UB).
        extern "C" fn before(_: Ctx, _: Server<'_>) {}

        let callbacks = ServerCallbacks::new();
        let slot = callbacks.slot(CB_START).unwrap();
        fill(slot);
        slot.invoke.store(before as *mut c_void, Relaxed);
        slot.drop.store(drop_kept as *mut c_void, Relaxed);
        callbacks.set(Callback::Start(start));

        // The slot calls `start`, not just some non-null fn: invoke it as
        // `call_server` does. `start` ignores its `Server`, so a null one is
        // never dereferenced. (A fn-pointer address comparison is unstable
        // under miri, which gives a fn a fresh address per cast site.)
        let invoke = slot.invoke.load(Relaxed);
        // SAFETY: the slot holds a `Callback::Start` fn, whose `Server` is a
        // `repr(transparent)` pointer it ignores; the ctx is the slot's.
        unsafe {
            core::mem::transmute::<*mut c_void, Invoke>(invoke)(
                slot.ctx.load(Relaxed),
                core::ptr::null_mut(),
            );
        }
        assert_eq!(CALLED.load(Relaxed), 1, "the slot calls start");
        assert_eq!(slot.ctx.load(Relaxed).addr(), 0x55, "the context stays");
        assert_eq!(
            KEPT_DROPPED.load(Relaxed),
            0,
            "nothing is freed under a thread calling the slot"
        );
        drop(callbacks);
        assert_eq!(KEPT_DROPPED.load(Relaxed), 1, "the slots free it once");
    }

    #[test]
    fn facts_read_and_clear_the_slots() {
        let state = ServerState::new();
        assert_eq!(native_fact(&state, OP_HAS_ON_CONNECT, 0), Some(0));
        fill(state.callbacks.slot(CB_CONNECT).unwrap());
        fill(state.callbacks.slot(CB_TASK).unwrap());
        fill(state.callbacks.slot(CB_DISPATCH_FUNC).unwrap());
        assert_eq!(native_fact(&state, OP_HAS_ON_CONNECT, 0), Some(1));
        assert_eq!(native_fact(&state, OP_HAS_ON_CLOSE, 0), Some(0));
        assert_eq!(native_fact(&state, OP_HAS_DISPATCH_FUNC, 0), Some(1));
        assert_eq!(
            native_fact(&state, OP_CALLBACK, u64::from(CB_TASK)),
            Some(1)
        );
        assert_eq!(
            native_fact(&state, OP_CALLBACK, u64::from(CB_DISPATCH_FUNC)),
            Some(0),
            "not a callback id"
        );
        // Only the four dispatch-only callbacks clear, as master.cc's switch did.
        let before = DROPPED.load(Relaxed);
        assert_eq!(
            native_fact(&state, OP_CLEAR_CALLBACK, u64::from(CB_TASK)),
            Some(0)
        );
        assert!(
            state.callbacks.slot(CB_TASK).unwrap().is_set(),
            "onTask stays"
        );
        assert_eq!(
            native_fact(&state, OP_CLEAR_CALLBACK, u64::from(CB_CONNECT)),
            Some(0)
        );
        assert!(
            !state.callbacks.slot(CB_CONNECT).unwrap().is_set(),
            "onConnect cleared"
        );
        assert_eq!(DROPPED.load(Relaxed), before + 1, "its box freed once");
        assert_eq!(native_fact(&state, crate::ops::OP_MODE, 0), None);
        drop(state);
        assert_eq!(
            DROPPED.load(Relaxed),
            before + 3,
            "the state frees the rest"
        );
    }

    #[test]
    fn header_enum_matches() {
        let text = include_str!("../../../native/include/swoole_rs_server_state.h");
        let body = text
            .split("SWR_SERVER_CB_CONNECT")
            .nth(1)
            .and_then(|t| t.split("};").next())
            .unwrap_or_default();
        let names: Vec<&str> = core::iter::once("SWR_SERVER_CB_CONNECT")
            .chain(body.split(',').map(str::trim).filter(|n| !n.is_empty()))
            .collect();
        let rust = [
            ("CONNECT", CB_CONNECT as usize),
            ("CLOSE", CB_CLOSE as usize),
            ("BUFFER_FULL", CB_BUFFER_FULL as usize),
            ("BUFFER_EMPTY", CB_BUFFER_EMPTY as usize),
            ("TASK", CB_TASK as usize),
            ("RECEIVE", CB_RECEIVE as usize),
            ("PACKET", CB_PACKET as usize),
            ("DISPATCH_FUNC", CB_DISPATCH_FUNC as usize),
            ("COUNT", CB_COUNT),
        ];
        for (name, index) in rust {
            let native = format!("SWR_SERVER_CB_{name}");
            assert_eq!(
                names.iter().position(|n| *n == native),
                Some(index),
                "{native}"
            );
        }
    }
}
