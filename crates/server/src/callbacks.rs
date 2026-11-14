//! `Server`'s callbacks: upstream's on* `std::function` members and `dispatch_func`.
//! Rust owns the slots, each typed by its signature. A slot holds `invoke(ctx, ...)`.
use crate::ConnectionRecord;
use crate::reactor_entry::RecvData;
use crate::view::Server;
use core::ffi::{c_int, c_void};
use core::marker::PhantomData;
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
    /// The slot this fills.
    #[must_use]
    pub fn slot(self) -> CallbackKind {
        match self {
            Self::Connect(_) => CallbackKind::Connect,
            Self::Close(_) => CallbackKind::Close,
            Self::BufferFull(_) => CallbackKind::BufferFull,
            Self::BufferEmpty(_) => CallbackKind::BufferEmpty,
            Self::Task(_) => CallbackKind::Task,
            Self::Receive(_) => CallbackKind::Receive,
            Self::Packet(_) => CallbackKind::Packet,
            Self::Start(_) => CallbackKind::Start,
            Self::BeforeShutdown(_) => CallbackKind::BeforeShutdown,
            Self::Shutdown(_) => CallbackKind::Shutdown,
            Self::ManagerStart(_) => CallbackKind::ManagerStart,
            Self::ManagerStop(_) => CallbackKind::ManagerStop,
            Self::WorkerError(_) => CallbackKind::WorkerError,
            Self::BeforeReload(_) => CallbackKind::BeforeReload,
            Self::AfterReload(_) => CallbackKind::AfterReload,
            Self::PipeMessage(_) => CallbackKind::PipeMessage,
            Self::WorkerStart(_) => CallbackKind::WorkerStart,
            Self::WorkerStop(_) => CallbackKind::WorkerStop,
            Self::WorkerExit(_) => CallbackKind::WorkerExit,
            Self::UserWorkerStart(_) => CallbackKind::UserWorkerStart,
            Self::Finish(_) => CallbackKind::Finish,
            Self::DispatchFunc(_) => CallbackKind::DispatchFunc,
        }
    }
}

/// `swr_v1_server_callback`: a slot as native code's view lays it out. `invoke` is
/// null or a fn of the signature the [`Slot`] holding this record names, and `ctx`
/// the context it takes first. `drop` is the native closure's destructor, which no
/// writer sets any more (Rust fns own no context, and the test fixtures' closures
/// own theirs); it stays for the layout.
#[repr(C)]
pub(crate) struct CallbackSlot {
    invoke: AtomicPtr<c_void>,
    ctx: AtomicPtr<c_void>,
    drop: AtomicPtr<c_void>,
}

impl CallbackSlot {
    const fn new() -> Self {
        Self {
            invoke: AtomicPtr::new(core::ptr::null_mut()),
            ctx: AtomicPtr::new(core::ptr::null_mut()),
            drop: AtomicPtr::new(core::ptr::null_mut()),
        }
    }

    #[inline]
    pub(crate) fn is_set(&self) -> bool {
        !self.invoke.load(Relaxed).is_null()
    }

    /// `serv->onX = nullptr`: empties the slot.
    pub(crate) fn clear(&self) {
        self.invoke.store(core::ptr::null_mut(), Relaxed);
        self.ctx.store(core::ptr::null_mut(), Relaxed);
    }
}

/// The fn types a [`Slot`] may hold: the signatures above, each an `extern "C"` fn
/// pointer. Only this module names the trait, and it implements it for those types
/// alone, so a `Slot<F>` holds a fn pointer of type `F`.
pub(crate) trait Signature: Copy {
    /// The fn's address, as the slot stores it.
    fn address(self) -> *mut c_void;
}

impl Signature for ServerFn {
    fn address(self) -> *mut c_void {
        self as *mut c_void
    }
}
impl Signature for EventFn {
    fn address(self) -> *mut c_void {
        self as *mut c_void
    }
}
impl Signature for ReceiveFn {
    fn address(self) -> *mut c_void {
        self as *mut c_void
    }
}
impl Signature for TaskFn {
    fn address(self) -> *mut c_void {
        self as *mut c_void
    }
}
impl Signature for MessageFn {
    fn address(self) -> *mut c_void {
        self as *mut c_void
    }
}
impl Signature for WorkerFn {
    fn address(self) -> *mut c_void {
        self as *mut c_void
    }
}
impl Signature for WorkerErrorFn {
    fn address(self) -> *mut c_void {
        self as *mut c_void
    }
}
impl Signature for DispatchFn {
    fn address(self) -> *mut c_void {
        self as *mut c_void
    }
}

/// One of a server's slots, of signature `F`: [`Slot::set`] stores only an `F`, and
/// native code's view assigns the slot its member of that signature.
#[repr(transparent)]
pub(crate) struct Slot<F> {
    raw: CallbackSlot,
    signature: PhantomData<F>,
}

impl<F: Signature> Slot<F> {
    const fn new() -> Self {
        Self {
            raw: CallbackSlot::new(),
            signature: PhantomData,
        }
    }

    /// `serv->onX = f` for a Rust `f`, which ignores its `ctx`. Only the fn changes:
    /// a context a native closure set stays, so a thread calling the slot meanwhile
    /// runs the old fn or the new one over a context that is live.
    fn set(&self, f: F) {
        self.raw.invoke.store(f.address(), Relaxed);
    }

    /// The fn the slot holds and the context it takes first, or `None` when empty.
    #[inline(always)]
    fn get(&self) -> Option<(F, Ctx)> {
        const {
            assert!(
                size_of::<F>() == size_of::<*mut c_void>(),
                "a slot holds a fn pointer"
            );
        };
        let invoke = self.raw.invoke.load(Relaxed);
        if invoke.is_null() {
            return None;
        }
        // SAFETY: a `Slot<F>` holds null or an `F`: `set` stores only an `F`'s address,
        // native code's view assigns the slot its member of `F`'s signature, and
        // `Signature` is implemented only for fn pointer types, of a pointer's size.
        let invoke = unsafe { core::mem::transmute_copy::<*mut c_void, F>(&invoke) };
        Some((invoke, self.raw.ctx.load(Relaxed)))
    }
}

/// Every slot of one server, in [`CallbackKind`] order: native code's view indexes
/// them by kind.
#[repr(C)]
pub(crate) struct ServerCallbacks {
    connect: Slot<EventFn>,
    close: Slot<EventFn>,
    buffer_full: Slot<EventFn>,
    buffer_empty: Slot<EventFn>,
    task: Slot<TaskFn>,
    receive: Slot<ReceiveFn>,
    packet: Slot<ReceiveFn>,
    start: Slot<ServerFn>,
    before_shutdown: Slot<ServerFn>,
    shutdown: Slot<ServerFn>,
    manager_start: Slot<ServerFn>,
    manager_stop: Slot<ServerFn>,
    worker_error: Slot<WorkerErrorFn>,
    before_reload: Slot<ServerFn>,
    after_reload: Slot<ServerFn>,
    pipe_message: Slot<MessageFn>,
    worker_start: Slot<WorkerFn>,
    worker_stop: Slot<WorkerFn>,
    worker_exit: Slot<WorkerFn>,
    user_worker_start: Slot<UserWorkerFn>,
    finish: Slot<TaskFn>,
    dispatch_func: Slot<DispatchFn>,
}

const _: () = assert!(
    size_of::<ServerCallbacks>() == SLOTS * size_of::<CallbackSlot>()
        && core::mem::offset_of!(ServerCallbacks, dispatch_func)
            == CallbackKind::DispatchFunc as usize * size_of::<CallbackSlot>()
        && core::mem::offset_of!(ServerCallbacks, pipe_message)
            == CallbackKind::PipeMessage as usize * size_of::<CallbackSlot>(),
    "the slots are laid out in kind order"
);

impl ServerCallbacks {
    pub(crate) const fn new() -> Self {
        Self {
            connect: Slot::new(),
            close: Slot::new(),
            buffer_full: Slot::new(),
            buffer_empty: Slot::new(),
            task: Slot::new(),
            receive: Slot::new(),
            packet: Slot::new(),
            start: Slot::new(),
            before_shutdown: Slot::new(),
            shutdown: Slot::new(),
            manager_start: Slot::new(),
            manager_stop: Slot::new(),
            worker_error: Slot::new(),
            before_reload: Slot::new(),
            after_reload: Slot::new(),
            pipe_message: Slot::new(),
            worker_start: Slot::new(),
            worker_stop: Slot::new(),
            worker_exit: Slot::new(),
            user_worker_start: Slot::new(),
            finish: Slot::new(),
            dispatch_func: Slot::new(),
        }
    }

    /// `serv->onX = f` for the slot and fn `callback` names.
    pub(crate) fn set(&self, callback: Callback) {
        match callback {
            Callback::Connect(f) => self.connect.set(f),
            Callback::Close(f) => self.close.set(f),
            Callback::BufferFull(f) => self.buffer_full.set(f),
            Callback::BufferEmpty(f) => self.buffer_empty.set(f),
            Callback::Task(f) => self.task.set(f),
            Callback::Receive(f) => self.receive.set(f),
            Callback::Packet(f) => self.packet.set(f),
            Callback::Start(f) => self.start.set(f),
            Callback::BeforeShutdown(f) => self.before_shutdown.set(f),
            Callback::Shutdown(f) => self.shutdown.set(f),
            Callback::ManagerStart(f) => self.manager_start.set(f),
            Callback::ManagerStop(f) => self.manager_stop.set(f),
            Callback::WorkerError(f) => self.worker_error.set(f),
            Callback::BeforeReload(f) => self.before_reload.set(f),
            Callback::AfterReload(f) => self.after_reload.set(f),
            Callback::PipeMessage(f) => self.pipe_message.set(f),
            Callback::WorkerStart(f) => self.worker_start.set(f),
            Callback::WorkerStop(f) => self.worker_stop.set(f),
            Callback::WorkerExit(f) => self.worker_exit.set(f),
            Callback::UserWorkerStart(f) => self.user_worker_start.set(f),
            Callback::Finish(f) => self.finish.set(f),
            Callback::DispatchFunc(f) => self.dispatch_func.set(f),
        }
    }

    /// Slot `kind`, whatever its signature: whether it is set, and clearing it.
    #[inline]
    pub(crate) fn slot(&self, kind: CallbackKind) -> &CallbackSlot {
        match kind {
            CallbackKind::Connect => &self.connect.raw,
            CallbackKind::Close => &self.close.raw,
            CallbackKind::BufferFull => &self.buffer_full.raw,
            CallbackKind::BufferEmpty => &self.buffer_empty.raw,
            CallbackKind::Task => &self.task.raw,
            CallbackKind::Receive => &self.receive.raw,
            CallbackKind::Packet => &self.packet.raw,
            CallbackKind::Start => &self.start.raw,
            CallbackKind::BeforeShutdown => &self.before_shutdown.raw,
            CallbackKind::Shutdown => &self.shutdown.raw,
            CallbackKind::ManagerStart => &self.manager_start.raw,
            CallbackKind::ManagerStop => &self.manager_stop.raw,
            CallbackKind::WorkerError => &self.worker_error.raw,
            CallbackKind::BeforeReload => &self.before_reload.raw,
            CallbackKind::AfterReload => &self.after_reload.raw,
            CallbackKind::PipeMessage => &self.pipe_message.raw,
            CallbackKind::WorkerStart => &self.worker_start.raw,
            CallbackKind::WorkerStop => &self.worker_stop.raw,
            CallbackKind::WorkerExit => &self.worker_exit.raw,
            CallbackKind::UserWorkerStart => &self.user_worker_start.raw,
            CallbackKind::Finish => &self.finish.raw,
            CallbackKind::DispatchFunc => &self.dispatch_func.raw,
        }
    }

    /// `serv->dispatch_func(serv, conn, send)`; 0 when the slot is empty.
    pub(crate) fn dispatch(
        &self,
        serv: Server<'_>,
        conn: Option<&ConnectionRecord>,
        send: Option<&RecvData>,
    ) -> c_int {
        self.dispatch_func
            .get()
            .map_or(0, |(invoke, ctx)| invoke(ctx, serv, conn, send))
    }

    /// `serv->onX(serv)` for a `(Server *)` slot; nothing when it is empty.
    pub(crate) fn call_server(&self, slot: ServerSlot, serv: Server<'_>) {
        let slot = match slot {
            ServerSlot::Start => &self.start,
            ServerSlot::BeforeShutdown => &self.before_shutdown,
            ServerSlot::Shutdown => &self.shutdown,
            ServerSlot::ManagerStart => &self.manager_start,
            ServerSlot::ManagerStop => &self.manager_stop,
            ServerSlot::BeforeReload => &self.before_reload,
            ServerSlot::AfterReload => &self.after_reload,
        };
        if let Some((invoke, ctx)) = slot.get() {
            invoke(ctx, serv);
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
pub(crate) enum ServerSlot {
    Start,
    BeforeShutdown,
    Shutdown,
    ManagerStart,
    ManagerStop,
    BeforeReload,
    AfterReload,
}

/// The slots of a `void (Server *, DataInfo *)` signature ([`EventFn`]).
#[cfg_attr(
    not(all(feature = "upstream-symbols", not(test))),
    expect(dead_code, reason = "only the hybrid raises connect and buffer events")
)]
#[derive(Clone, Copy)]
pub(crate) enum EventSlot {
    Connect,
    Close,
    BufferFull,
    BufferEmpty,
}

/// The slots of an `int (Server *, RecvData *)` signature ([`ReceiveFn`]).
#[derive(Clone, Copy)]
pub(crate) enum ReceiveSlot {
    Receive,
    Packet,
}

/// The typed calls the server makes in every build (see the impl below).
impl ServerCallbacks {
    /// `serv->onX(serv, info)` for an event slot; nothing when it is empty. Inlined,
    /// so a constant slot folds to its address as the caller's own lookup did.
    #[inline(always)]
    pub(crate) fn event(&self, slot: EventSlot, serv: Server<'_>, info: &DataHead) {
        let slot = match slot {
            EventSlot::Connect => &self.connect,
            EventSlot::Close => &self.close,
            EventSlot::BufferFull => &self.buffer_full,
            EventSlot::BufferEmpty => &self.buffer_empty,
        };
        if let Some((invoke, ctx)) = slot.get() {
            invoke(ctx, serv, info);
        }
    }

    /// `serv->onReceive(serv, data)` or `onPacket(serv, data)`; -1 when the slot is
    /// empty. Inlined, as [`ServerCallbacks::event`] is: every request calls it.
    #[inline(always)]
    pub(crate) fn receive(&self, slot: ReceiveSlot, serv: Server<'_>, data: &RecvData) -> c_int {
        let slot = match slot {
            ReceiveSlot::Receive => &self.receive,
            ReceiveSlot::Packet => &self.packet,
        };
        slot.get()
            .map_or(-1, |(invoke, ctx)| invoke(ctx, serv, data))
    }
}

/// The slots of an `int (Server *, EventData *)` signature ([`TaskFn`]).
#[cfg(all(feature = "upstream-symbols", not(test)))]
#[derive(Clone, Copy)]
pub(crate) enum TaskSlot {
    Task,
    Finish,
}

/// The slots of a `void (Server *, Worker *)` signature over a worker this thread
/// or process runs ([`WorkerFn`]).
#[cfg(all(feature = "upstream-symbols", not(test)))]
#[derive(Clone, Copy)]
pub(crate) enum WorkerSlot {
    Start,
    Stop,
    Exit,
}

/// The typed calls. Each names its slots by a type that admits no other, and every
/// argument is a live handle or a reference its caller lends for the call.
#[cfg(all(feature = "upstream-symbols", not(test)))]
impl ServerCallbacks {
    /// `serv->onTask(serv, task)` or `onFinish(serv, task)`; -1 when the slot is
    /// empty. A caller that discards the result (onFinish) still calls through the
    /// int type the slot holds.
    pub(crate) fn task(&self, slot: TaskSlot, serv: Server<'_>, task: Packed<'_>) -> c_int {
        let slot = match slot {
            TaskSlot::Task => &self.task,
            TaskSlot::Finish => &self.finish,
        };
        slot.get()
            .map_or(-1, |(invoke, ctx)| invoke(ctx, serv, task))
    }

    /// `serv->onPipeMessage(serv, message)`; nothing when the slot is empty.
    pub(crate) fn pipe_message(&self, serv: Server<'_>, message: Packed<'_>) {
        if let Some((invoke, ctx)) = self.pipe_message.get() {
            invoke(ctx, serv, message);
        }
    }

    /// `serv->onWorkerX(serv, worker)` for a worker slot; nothing when it is empty.
    pub(crate) fn worker(&self, slot: WorkerSlot, serv: Server<'_>, worker: Worker<'_>) {
        let slot = match slot {
            WorkerSlot::Start => &self.worker_start,
            WorkerSlot::Stop => &self.worker_stop,
            WorkerSlot::Exit => &self.worker_exit,
        };
        if let Some((invoke, ctx)) = slot.get() {
            invoke(ctx, serv, worker);
        }
    }

    /// `serv->onUserWorkerStart(serv, worker)`; nothing when the slot is empty.
    pub(crate) fn user_worker_start(&self, serv: Server<'_>, worker: Worker<'_>) {
        if let Some((invoke, ctx)) = self.user_worker_start.get() {
            invoke(ctx, serv, worker);
        }
    }

    /// `serv->onWorkerError(serv, worker, status)`; nothing when the slot is empty.
    pub(crate) fn worker_error(&self, serv: Server<'_>, worker: Worker<'_>, status: &ExitStatus) {
        if let Some((invoke, ctx)) = self.worker_error.get() {
            invoke(ctx, serv, worker, status);
        }
    }
}

#[cfg(all(feature = "upstream-symbols", not(test)))]
impl crate::ServerState {
    /// The first of the slots of the state at `state`, which `Server`'s callbacks
    /// pointer heads: an address within the state, read through no reference.
    pub(crate) fn callbacks_ptr(state: *mut Self) -> *mut c_void {
        state
            .wrapping_byte_add(core::mem::offset_of!(Self, callbacks))
            .cast()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use core::sync::atomic::AtomicU32;

    unsafe extern "C" fn invoke() {}

    /// Sets `slot` to a fn that owns no context, as a callback the script set.
    pub(crate) fn on(slot: &CallbackSlot) {
        slot.invoke.store(invoke as *mut c_void, Relaxed);
    }

    fn fill(slot: &CallbackSlot) {
        on(slot);
        slot.ctx
            .store(core::ptr::without_provenance_mut(0x55), Relaxed);
    }

    #[test]
    fn a_rust_fn_replaces_the_fn_and_keeps_the_context() {
        static CALLED: AtomicU32 = AtomicU32::new(0);
        // Records that the slot ran it, with the slot's context; it ignores the server.
        extern "C" fn start(ctx: Ctx, _: Server<'_>) {
            assert_eq!(ctx.addr(), 0x55, "the slot's ctx");
            CALLED.fetch_add(1, Relaxed);
        }
        // The fn the slot holds before replacement, of `start`'s signature.
        extern "C" fn before(_: Ctx, _: Server<'_>) {}

        let callbacks = ServerCallbacks::new();
        let slot = callbacks.slot(CallbackKind::Start);
        fill(slot);
        callbacks.start.set(before);
        callbacks.set(Callback::Start(start));

        // SAFETY: `start` ignores its server, so a null one is never dereferenced.
        let serv = unsafe { Server::from_raw(core::ptr::null_mut()) };
        callbacks.call_server(ServerSlot::Start, serv);
        assert_eq!(CALLED.load(Relaxed), 1, "the slot calls start");
        assert_eq!(slot.ctx.load(Relaxed).addr(), 0x55, "the context stays");
    }

    #[test]
    fn clearing_a_slot_empties_it_alone() {
        let state = crate::ServerState::new();
        fill(state.callbacks.slot(CallbackKind::Connect));
        fill(state.callbacks.slot(CallbackKind::Task));
        let connect = state.callbacks.slot(CallbackKind::Connect);
        connect.clear();
        assert!(!connect.is_set(), "onConnect cleared");
        assert!(connect.ctx.load(Relaxed).is_null(), "and its context");
        assert!(
            state.callbacks.slot(CallbackKind::Task).is_set(),
            "onTask stays"
        );
    }
}
