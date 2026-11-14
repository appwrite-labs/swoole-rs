//! `Server`'s callbacks: upstream's on* `std::function` members and `dispatch_func`.
//! Rust owns the slots, and `Server` binds each member to its slot through
//! `detail::Callback` (`native/include/swoole_rs_server_view.h`). That view boxes
//! whatever native code assigns, so the slot holds `invoke(ctx, ...)` and the
//! `drop(ctx)` that frees the box. Rust answers the presence facts and clears slots;
//! native code still makes the calls.
use crate::ServerState;
use crate::ops::{
    OP_CALLBACK, OP_CLEAR_CALLBACK, OP_HAS_DISPATCH_FUNC, OP_HAS_ON_CLOSE, OP_HAS_ON_CONNECT,
};
use core::ffi::c_void;
use core::sync::atomic::AtomicPtr;
use core::sync::atomic::Ordering::Relaxed;

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

    /// `serv->onX = f` for a Rust `f` of the slot's signature, called with a
    /// null `ctx`: frees what the slot held first.
    pub(crate) fn set(&self, invoke: *const c_void) {
        self.clear();
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
