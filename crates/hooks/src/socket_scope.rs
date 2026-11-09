//! Scoped coroutine socket state. Copies are independent snapshots of the same
//! borrowed socket; restoring a scope never owns or destroys that socket.
use crate::co_socket::{CoSocket, Protocol};
use core::ffi::c_int;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ProtocolScope {
    pub socket: *mut CoSocket,
    pub eof: bool,
    pub length: bool,
    pub protocol: Protocol,
}

/// # Safety
/// socket is a live, synchronized coroutine socket for the scope's lifetime.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_protocol_scope(socket: *mut CoSocket) -> ProtocolScope {
    // SAFETY: all fields belong to the caller's live socket.
    let record = unsafe { &*socket };
    ProtocolScope {
        socket,
        eof: record.open_eof_check,
        length: record.open_length_check,
        protocol: record.protocol,
    }
}

/// # Safety
/// scope holds a snapshot of a still-live, synchronized socket.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_protocol_restore(scope: *const ProtocolScope) {
    // SAFETY: the scope is live and copied before mutating its borrowed socket.
    let saved = unsafe { *scope };
    // SAFETY: its socket remains live and exclusive for restoration.
    let socket = unsafe { &mut *saved.socket };
    socket.open_eof_check = saved.eof;
    socket.open_length_check = saved.length;
    socket.protocol = saved.protocol;
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TimeoutScope {
    pub socket: *mut CoSocket,
    pub timeout: f64,
    pub kind: c_int,
    // Preserve the original public header's sizeof(enum-array) storage extent.
    pub original: [f64; 16],
    pub startup: f64,
}

/// Saves the selected timeouts before applying the temporary value.
/// # Safety
/// socket and its network record stay live and synchronized through restoration.
#[unsafe(no_mangle)]
#[expect(
    clippy::float_cmp,
    reason = "the original compares saved timeouts exactly"
)]
pub unsafe extern "C" fn swr_v1_hook_timeout_scope(
    socket: *mut CoSocket,
    timeout: f64,
    kind: c_int,
) -> TimeoutScope {
    let mut scope = TimeoutScope {
        socket,
        timeout,
        kind,
        original: [0.0; 16],
        startup: 0.0,
    };
    if timeout == 0.0 {
        return scope;
    }
    // SAFETY: the caller lends a live coroutine socket and network record.
    let network = unsafe { (*socket).socket };
    // SAFETY: the live synchronized record, exclusively ours until this returns.
    let timeouts = unsafe { &mut (*network).timeouts };
    for (original, bit) in scope.original.iter_mut().zip([1, 2, 4, 8]) {
        if kind & bit != 0 {
            let value = timeouts.get(bit).unwrap_or_default();
            *original = value;
            if timeout != value {
                timeouts.set(timeout, bit);
            }
        }
    }
    scope
}

/// # Safety
/// scope and the socket it borrowed remain live and synchronized.
#[unsafe(no_mangle)]
#[expect(
    clippy::float_cmp,
    reason = "the original compares saved timeouts exactly"
)]
pub unsafe extern "C" fn swr_v1_hook_timeout_restore(scope: *const TimeoutScope) {
    // SAFETY: the scope is live; copy before mutating its socket.
    let saved = unsafe { *scope };
    if saved.timeout == 0.0 {
        return;
    }
    // SAFETY: the borrowed network record stays live through restoration.
    let network = unsafe { (*saved.socket).socket };
    // SAFETY: the scope's still-live synchronized timeout group, ours until this returns.
    let timeouts = unsafe { &mut (*network).timeouts };
    for (original, bit) in saved.original.into_iter().zip([1, 2, 4, 8]) {
        if saved.kind & bit != 0 && saved.timeout != original {
            timeouts.set(original, bit);
        }
    }
}

#[cfg(target_pointer_width = "64")]
const _: () = {
    assert!(
        core::mem::size_of::<ProtocolScope>() == 80,
        "native protocol scope size"
    );
    assert!(
        core::mem::offset_of!(ProtocolScope, protocol) == 16,
        "native protocol snapshot offset"
    );
    assert!(
        core::mem::size_of::<TimeoutScope>() == 160,
        "native timeout scope size"
    );
    assert!(
        core::mem::offset_of!(TimeoutScope, original) == 24,
        "native saved-timeout offset"
    );
    assert!(
        core::mem::offset_of!(TimeoutScope, startup) == 152,
        "native controller clock offset"
    );
};
