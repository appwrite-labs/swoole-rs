//! `Server`'s callbacks: upstream's on* `std::function` members and `dispatch_func`.
//! Rust owns the slots. A slot holds `invoke(ctx, ...)` and the `drop(ctx)` that
//! frees `ctx`. Rust clears slots;
//! native code still makes the calls.
use crate::reactor_entry::RecvData;
use crate::view::Server;
use crate::{ConnectionRecord, ServerState};
use core::ffi::{c_int, c_void};
use core::sync::atomic::AtomicPtr;
use core::sync::atomic::Ordering::Relaxed;
use swoole_rs_corebase::DataHead;
use swoole_rs_process::methods::ExitStatus;
use swoole_rs_process::task_file::Packed;
use swoole_rs_process::worker_record::Worker;

/// A server's slots. The first seven are also `OP_CALLBACK`'s
/// argument and the callback its logs name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum CallbackKind {
    Connect,
    Close,
    BufferFull,
    BufferEmpty,
    Task,
    Receive,
    Packet,
    Start,
    BeforeShutdown,
    Shutdown,
    ManagerStart,
    ManagerStop,
    WorkerError,
    BeforeReload,
    AfterReload,
    PipeMessage,
    WorkerStart,
    WorkerStop,
    WorkerExit,
    UserWorkerStart,
    Finish,
    DispatchFunc,
}

/// The number of slots.
const SLOTS: usize = CallbackKind::DispatchFunc as usize + 1;

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
pub type WorkerFn = extern "C" fn(Ctx, Server<'_>, Worker<'_>);
/// `void (Server *, Worker *, const ExitStatus &)`.
pub type WorkerErrorFn = extern "C" fn(Ctx, Server<'_>, Worker<'_>, &ExitStatus);
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
    /// The slot and the fn as the slot stores it.
    fn parts(self) -> (CallbackKind, *const c_void) {
        match self {
            Self::Connect(invoke) => (CallbackKind::Connect, invoke as *const c_void),
            Self::Close(invoke) => (CallbackKind::Close, invoke as *const c_void),
            Self::BufferFull(invoke) => (CallbackKind::BufferFull, invoke as *const c_void),
            Self::BufferEmpty(invoke) => (CallbackKind::BufferEmpty, invoke as *const c_void),
            Self::Task(invoke) => (CallbackKind::Task, invoke as *const c_void),
            Self::Receive(invoke) => (CallbackKind::Receive, invoke as *const c_void),
            Self::Packet(invoke) => (CallbackKind::Packet, invoke as *const c_void),
            Self::Start(invoke) => (CallbackKind::Start, invoke as *const c_void),
            Self::BeforeShutdown(invoke) => (CallbackKind::BeforeShutdown, invoke as *const c_void),
            Self::Shutdown(invoke) => (CallbackKind::Shutdown, invoke as *const c_void),
            Self::ManagerStart(invoke) => (CallbackKind::ManagerStart, invoke as *const c_void),
            Self::ManagerStop(invoke) => (CallbackKind::ManagerStop, invoke as *const c_void),
            Self::WorkerError(invoke) => (CallbackKind::WorkerError, invoke as *const c_void),
            Self::BeforeReload(invoke) => (CallbackKind::BeforeReload, invoke as *const c_void),
            Self::AfterReload(invoke) => (CallbackKind::AfterReload, invoke as *const c_void),
            Self::PipeMessage(invoke) => (CallbackKind::PipeMessage, invoke as *const c_void),
            Self::WorkerStart(invoke) => (CallbackKind::WorkerStart, invoke as *const c_void),
            Self::WorkerStop(invoke) => (CallbackKind::WorkerStop, invoke as *const c_void),
            Self::WorkerExit(invoke) => (CallbackKind::WorkerExit, invoke as *const c_void),
            Self::UserWorkerStart(invoke) => {
                (CallbackKind::UserWorkerStart, invoke as *const c_void)
            }
            Self::Finish(invoke) => (CallbackKind::Finish, invoke as *const c_void),
            Self::DispatchFunc(invoke) => (CallbackKind::DispatchFunc, invoke as *const c_void),
        }
    }

    /// The slot this fills.
    #[must_use]
    pub fn slot(self) -> CallbackKind {
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

    /// `serv->onX = nullptr`: empties the slot, then frees what it held.
    pub(crate) fn clear(&self) {
        self.invoke.store(core::ptr::null_mut(), Relaxed);
        let ctx = self.ctx.swap(core::ptr::null_mut(), Relaxed);
        let drop = self.drop.swap(core::ptr::null_mut(), Relaxed);
        // SAFETY: native code stored null or a `void (*)(void *) noexcept` paired with `ctx`.
        let drop = unsafe { core::mem::transmute::<*mut c_void, Option<DropFn>>(drop) };
        if let Some(drop) = drop {
            // SAFETY: the box is this slot's alone, and the slot no longer names it.
            unsafe { drop(ctx) };
        }
    }
}

/// Every slot of one server.
#[repr(C)]
pub(crate) struct ServerCallbacks([CallbackSlot; SLOTS]);

impl ServerCallbacks {
    /// `serv->dispatch_func(serv, conn, send)`; 0 when the slot is empty.
    pub(crate) fn dispatch(
        &self,
        serv: Server<'_>,
        conn: Option<&ConnectionRecord>,
        send: Option<&RecvData>,
    ) -> c_int {
        type Invoke = extern "C-unwind" fn(
            Ctx,
            Server<'_>,
            Option<&ConnectionRecord>,
            Option<&RecvData>,
        ) -> c_int;
        let Some(slot) = self.slot(CallbackKind::DispatchFunc) else {
            return 0;
        };
        let invoke = slot.invoke.load(Relaxed);
        // SAFETY: dispatch_func's slot holds null or an
        // `int (*)(void *, Server *, Connection *, SendData *)`.
        let invoke = unsafe { core::mem::transmute::<*mut c_void, Option<Invoke>>(invoke) };
        invoke.map_or(0, |invoke| invoke(slot.ctx.load(Relaxed), serv, conn, send))
    }

    pub(crate) const fn new() -> Self {
        Self([const { CallbackSlot::new() }; SLOTS])
    }

    pub(crate) fn slot(&self, slot: CallbackKind) -> Option<&CallbackSlot> {
        self.at(slot as u32)
    }

    fn at(&self, index: u32) -> Option<&CallbackSlot> {
        self.0.get(index as usize)
    }

    /// `serv->onX(serv)` for a `(Server *)` slot; nothing when it is empty.
    pub(crate) fn call_server(&self, slot: ServerSlot, serv: Server<'_>) {
        type Invoke = extern "C-unwind" fn(Ctx, Server<'_>);
        let Some(slot) = self.at(slot as u32) else {
            return;
        };
        let invoke = slot.invoke.load(Relaxed);
        // SAFETY: a `(Server *)` slot holds null or a `void (*)(void *, Server *)`.
        let invoke = unsafe { core::mem::transmute::<*mut c_void, Option<Invoke>>(invoke) };
        if let Some(invoke) = invoke {
            invoke(slot.ctx.load(Relaxed), serv);
        }
    }
}

/// The slots of a `void (Server *)` signature ([`ServerFn`]).
#[cfg_attr(
    not(all(feature = "upstream-symbols", not(test))),
    expect(
        dead_code,
        reason = "only the hybrid raises the manager, reload and shutdown hooks"
    )
)]
#[derive(Clone, Copy)]
#[repr(u32)]
pub(crate) enum ServerSlot {
    Start = CallbackKind::Start as u32,
    BeforeShutdown = CallbackKind::BeforeShutdown as u32,
    Shutdown = CallbackKind::Shutdown as u32,
    ManagerStart = CallbackKind::ManagerStart as u32,
    ManagerStop = CallbackKind::ManagerStop as u32,
    BeforeReload = CallbackKind::BeforeReload as u32,
    AfterReload = CallbackKind::AfterReload as u32,
}

/// The slots of a `void (Server *, DataInfo *)` signature ([`EventFn`]).
#[cfg_attr(
    not(all(feature = "upstream-symbols", not(test))),
    expect(dead_code, reason = "only the hybrid raises connect and buffer events")
)]
#[derive(Clone, Copy)]
#[repr(u32)]
pub(crate) enum EventSlot {
    Connect = CallbackKind::Connect as u32,
    Close = CallbackKind::Close as u32,
    BufferFull = CallbackKind::BufferFull as u32,
    BufferEmpty = CallbackKind::BufferEmpty as u32,
}

/// The slots of an `int (Server *, RecvData *)` signature ([`ReceiveFn`]).
#[derive(Clone, Copy)]
#[repr(u32)]
pub(crate) enum ReceiveSlot {
    Receive = CallbackKind::Receive as u32,
    Packet = CallbackKind::Packet as u32,
}

/// The typed calls the server makes in every build (see the impl below).
impl ServerCallbacks {
    /// `serv->onX(serv, info)` for an event slot; nothing when it is empty. Inlined,
    /// so a constant slot folds to its address as the caller's own lookup did.
    #[inline(always)]
    pub(crate) fn event(&self, slot: EventSlot, serv: Server<'_>, info: &DataHead) {
        type Invoke = extern "C-unwind" fn(Ctx, Server<'_>, &DataHead);
        let Some(slot) = self.at(slot as u32) else {
            return;
        };
        let invoke = slot.invoke.load(Relaxed);
        // SAFETY: an event slot holds null or a `void (*)(void *, Server *, DataInfo *)`.
        let invoke = unsafe { core::mem::transmute::<*mut c_void, Option<Invoke>>(invoke) };
        if let Some(invoke) = invoke {
            invoke(slot.ctx.load(Relaxed), serv, info);
        }
    }

    /// `serv->onReceive(serv, data)` or `onPacket(serv, data)`; -1 when the slot is
    /// empty. Inlined, as [`ServerCallbacks::event`] is: every request calls it.
    #[inline(always)]
    pub(crate) fn receive(&self, slot: ReceiveSlot, serv: Server<'_>, data: &RecvData) -> c_int {
        type Invoke = extern "C-unwind" fn(Ctx, Server<'_>, &RecvData) -> c_int;
        let Some(slot) = self.at(slot as u32) else {
            return -1;
        };
        let invoke = slot.invoke.load(Relaxed);
        // SAFETY: a receive slot holds null or an `int (*)(void *, Server *, RecvData *)`.
        let invoke = unsafe { core::mem::transmute::<*mut c_void, Option<Invoke>>(invoke) };
        invoke.map_or(-1, |invoke| invoke(slot.ctx.load(Relaxed), serv, data))
    }
}

/// The slots of an `int (Server *, EventData *)` signature ([`TaskFn`]).
#[cfg(all(feature = "upstream-symbols", not(test)))]
#[derive(Clone, Copy)]
#[repr(u32)]
pub(crate) enum TaskSlot {
    Task = CallbackKind::Task as u32,
    Finish = CallbackKind::Finish as u32,
}

/// The slots of a `void (Server *, Worker *)` signature over a worker this thread
/// or process runs ([`WorkerFn`]).
#[cfg(all(feature = "upstream-symbols", not(test)))]
#[derive(Clone, Copy)]
#[repr(u32)]
pub(crate) enum WorkerSlot {
    Start = CallbackKind::WorkerStart as u32,
    Stop = CallbackKind::WorkerStop as u32,
    Exit = CallbackKind::WorkerExit as u32,
}

/// The typed calls. A slot's index fixes its signature: native code assigns the
/// slot its typed member, and [`ServerCallbacks::set`] a [`Callback`] of the slot's
/// own fn type. Each call below names its slots by a type that admits no other, so
/// the one transmute in it is to the signature those slots hold, and every argument
/// is a live handle or a reference its caller lends for the call. The fn types are
/// [`Callback`]'s but `C-unwind`, as a callback native code assigned may unwind.
#[cfg(all(feature = "upstream-symbols", not(test)))]
impl ServerCallbacks {
    /// `serv->onTask(serv, task)` or `onFinish(serv, task)`; -1 when the slot is
    /// empty. A caller that discards the result (onFinish) still calls through the
    /// int type the slot holds.
    pub(crate) fn task(&self, slot: TaskSlot, serv: Server<'_>, task: Packed<'_>) -> c_int {
        type Invoke = extern "C-unwind" fn(Ctx, Server<'_>, Packed<'_>) -> c_int;
        let Some(slot) = self.at(slot as u32) else {
            return -1;
        };
        let invoke = slot.invoke.load(Relaxed);
        // SAFETY: a task slot holds null or an `int (*)(void *, Server *, EventData *)`.
        let invoke = unsafe { core::mem::transmute::<*mut c_void, Option<Invoke>>(invoke) };
        invoke.map_or(-1, |invoke| invoke(slot.ctx.load(Relaxed), serv, task))
    }

    /// `serv->onPipeMessage(serv, message)`; nothing when the slot is empty.
    pub(crate) fn pipe_message(&self, serv: Server<'_>, message: Packed<'_>) {
        type Invoke = extern "C-unwind" fn(Ctx, Server<'_>, Packed<'_>);
        let Some(slot) = self.slot(CallbackKind::PipeMessage) else {
            return;
        };
        let invoke = slot.invoke.load(Relaxed);
        // SAFETY: onPipeMessage's slot holds null or a `void (*)(void *, Server *, EventData *)`.
        let invoke = unsafe { core::mem::transmute::<*mut c_void, Option<Invoke>>(invoke) };
        if let Some(invoke) = invoke {
            invoke(slot.ctx.load(Relaxed), serv, message);
        }
    }

    /// `serv->onWorkerX(serv, worker)` for a worker slot; nothing when it is empty.
    pub(crate) fn worker(&self, slot: WorkerSlot, serv: Server<'_>, worker: Worker<'_>) {
        type Invoke = extern "C-unwind" fn(Ctx, Server<'_>, Worker<'_>);
        let Some(slot) = self.at(slot as u32) else {
            return;
        };
        let invoke = slot.invoke.load(Relaxed);
        // SAFETY: a worker slot holds null or a `void (*)(void *, Server *, Worker *)`.
        let invoke = unsafe { core::mem::transmute::<*mut c_void, Option<Invoke>>(invoke) };
        if let Some(invoke) = invoke {
            invoke(slot.ctx.load(Relaxed), serv, worker);
        }
    }

    /// `serv->onUserWorkerStart(serv, worker)`; nothing when the slot is empty.
    pub(crate) fn user_worker_start(&self, serv: Server<'_>, worker: Worker<'_>) {
        type Invoke = extern "C-unwind" fn(Ctx, Server<'_>, Worker<'_>);
        let Some(slot) = self.slot(CallbackKind::UserWorkerStart) else {
            return;
        };
        let invoke = slot.invoke.load(Relaxed);
        // SAFETY: onUserWorkerStart's slot holds null or a `void (*)(void *, Server *, Worker *)`.
        let invoke = unsafe { core::mem::transmute::<*mut c_void, Option<Invoke>>(invoke) };
        if let Some(invoke) = invoke {
            invoke(slot.ctx.load(Relaxed), serv, worker);
        }
    }

    /// `serv->onWorkerError(serv, worker, status)`; nothing when the slot is empty.
    pub(crate) fn worker_error(&self, serv: Server<'_>, worker: Worker<'_>, status: &ExitStatus) {
        type Invoke = extern "C-unwind" fn(Ctx, Server<'_>, Worker<'_>, &ExitStatus);
        let Some(slot) = self.slot(CallbackKind::WorkerError) else {
            return;
        };
        let invoke = slot.invoke.load(Relaxed);
        // SAFETY: onWorkerError's slot holds null or a
        // `void (*)(void *, Server *, Worker *, const ExitStatus *)`.
        let invoke = unsafe { core::mem::transmute::<*mut c_void, Option<Invoke>>(invoke) };
        if let Some(invoke) = invoke {
            invoke(slot.ctx.load(Relaxed), serv, worker, status);
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
    /// The first of the slots of the state at `state`, which `Server`'s callbacks
    /// pointer heads: an address within the state, read through no reference.
    pub(crate) fn callbacks_ptr(state: *mut ServerState) -> *mut c_void {
        state
            .wrapping_byte_add(core::mem::offset_of!(ServerState, callbacks))
            .cast()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use core::sync::atomic::AtomicU32;

    static DROPPED: AtomicU32 = AtomicU32::new(0);

    unsafe extern "C" fn count_drop(ctx: *mut c_void) {
        assert_eq!(ctx.addr(), 0x55, "the slot's ctx");
        DROPPED.fetch_add(1, Relaxed);
    }

    unsafe extern "C" fn invoke() {}

    /// Sets `slot` to a fn that owns no context, as a callback the script set.
    pub(crate) fn on(slot: &CallbackSlot) {
        slot.set(invoke as *const c_void);
    }

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
        let slot = callbacks.slot(CallbackKind::Start).unwrap();
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
    fn clearing_a_slot_frees_its_box_once() {
        let state = ServerState::new();
        fill(state.callbacks.slot(CallbackKind::Connect).unwrap());
        fill(state.callbacks.slot(CallbackKind::Task).unwrap());
        fill(state.callbacks.slot(CallbackKind::DispatchFunc).unwrap());
        let before = DROPPED.load(Relaxed);
        let connect = state.callbacks.slot(CallbackKind::Connect).unwrap();
        connect.clear();
        assert!(!connect.is_set(), "onConnect cleared");
        assert!(
            state.callbacks.slot(CallbackKind::Task).unwrap().is_set(),
            "onTask stays"
        );
        assert_eq!(DROPPED.load(Relaxed), before + 1, "its box freed once");
        drop(state);
        assert_eq!(
            DROPPED.load(Relaxed),
            before + 3,
            "the state frees the rest"
        );
    }
}
