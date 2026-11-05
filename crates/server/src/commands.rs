//! Command names, metadata, handler custody and outstanding reply callbacks.
//! Native callable objects are opaque values: Rust never invokes or destroys a
//! callable, so a Zend bailout cannot cross a Rust frame. Native teardown drains
//! these tables before freeing the server state.
use crate::ServerState;
use core::ffi::c_void;
use std::collections::BTreeMap;

struct Command {
    id: i32,
    accepted_types: i32,
}

pub(crate) struct Commands {
    named: BTreeMap<Vec<u8>, Command>,
    handlers: BTreeMap<i32, usize>,
    callbacks: BTreeMap<i64, usize>,
    next_request: i64,
}

impl Commands {
    pub(crate) const fn new() -> Self {
        Self {
            named: BTreeMap::new(),
            handlers: BTreeMap::new(),
            callbacks: BTreeMap::new(),
            next_request: 1,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct CommandInfo {
    pub name: *const u8,
    pub length: usize,
    pub id: i32,
    pub accepted_types: i32,
}

/// # Safety
/// `state` is live and the byte range is readable. A successful insertion takes
/// custody of `handler`; native teardown must drain and destroy it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_server_command_add(
    state: *const ServerState,
    name: *const u8,
    length: usize,
    id: i32,
    accepted_types: i32,
    handler: *mut c_void,
) -> bool {
    // SAFETY: the caller lends a live server.
    let state = unsafe { &*state };
    // SAFETY: the caller lends the named byte range, including embedded NULs.
    let name = unsafe { core::slice::from_raw_parts(name, length) };
    let mut commands = state
        .commands
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if commands.named.contains_key(name) {
        return false;
    }
    if !handler.is_null() {
        commands.handlers.insert(id, handler.expose_provenance());
    }
    commands
        .named
        .insert(name.to_vec(), Command { id, accepted_types });
    true
}

/// # Safety
/// The server and input bytes are live; `output` is writable. On success its
/// name is borrowed until the server's command table is destroyed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_server_command_find(
    state: *const ServerState,
    name: *const u8,
    length: usize,
    output: *mut CommandInfo,
) -> bool {
    // SAFETY: the server is live for this leaf lookup.
    let state = unsafe { &*state };
    // SAFETY: the caller lends exactly this input byte range.
    let name = unsafe { core::slice::from_raw_parts(name, length) };
    let commands = state
        .commands
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some((name, command)) = commands.named.get_key_value(name) else {
        return false;
    };
    // SAFETY: output is writable; stored names never change after insertion.
    unsafe {
        output.write(CommandInfo {
            name: name.as_ptr(),
            length: name.len(),
            id: command.id,
            accepted_types: command.accepted_types,
        });
    };
    true
}

/// Attach a native empty callable before its copying assignment, matching the
/// original map's publication/reentry order without a native constructor in Rust.
/// # Safety
/// `state` is live and takes custody of the non-null native callable `handler`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_server_command_attach(
    state: *const ServerState,
    id: i32,
    handler: *mut c_void,
) {
    // SAFETY: the caller lends a live Server state.
    let state = unsafe { &*state };
    state
        .commands
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .handlers
        .insert(id, handler.expose_provenance());
}

/// # Safety
/// `state` is live. The returned native callable is a loan; no table lock or
/// Rust reference remains live while native code invokes it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_server_command_handler(
    state: *const ServerState,
    id: i32,
) -> *mut c_void {
    // SAFETY: the caller lends a live server.
    let state = unsafe { &*state };
    let commands = state
        .commands
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    core::ptr::with_exposed_provenance_mut(commands.handlers.get(&id).copied().unwrap_or(0))
}

/// # Safety
/// The live server takes custody of `callback`, which native code must later
/// take (or drain) and destroy. Request ids wrap as the underlying counter does.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_server_command_request(
    state: *const ServerState,
    callback: *mut c_void,
) -> i64 {
    // SAFETY: the caller lends a live server.
    let state = unsafe { &*state };
    let mut commands = state
        .commands
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let id = commands.next_request;
    commands.next_request = id.wrapping_add(1);
    commands.callbacks.insert(id, callback.expose_provenance());
    id
}

/// # Safety
/// `state` is live. `take` transfers custody of the result to native code;
/// otherwise the result is a loan valid until the corresponding take.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_server_command_callback(
    state: *const ServerState,
    id: i64,
    take: bool,
) -> *mut c_void {
    // SAFETY: the caller lends a live server.
    let state = unsafe { &*state };
    let mut commands = state
        .commands
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let value = if take {
        commands.callbacks.remove(&id)
    } else {
        commands.callbacks.get(&id).copied()
    };
    core::ptr::with_exposed_provenance_mut(value.unwrap_or(0))
}

/// # Safety
/// The server is live and its command table is quiescent. `output` is writable;
/// the returned name remains borrowed until teardown.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_server_command_at(
    state: *const ServerState,
    index: usize,
    output: *mut CommandInfo,
) -> bool {
    // SAFETY: the caller lends a live server.
    let state = unsafe { &*state };
    let commands = state
        .commands
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some((name, command)) = commands.named.iter().nth(index) else {
        return false;
    };
    // SAFETY: the caller supplies writable output and the table owns this name.
    unsafe {
        output.write(CommandInfo {
            name: name.as_ptr(),
            length: name.len(),
            id: command.id,
            accepted_types: command.accepted_types,
        });
    };
    true
}

/// # Safety
/// Native teardown exclusively owns the live server. Each returned pointer is
/// transferred exactly once for native destruction, outside the Rust frame.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_server_command_drain(
    state: *const ServerState,
    callbacks: bool,
) -> *mut c_void {
    // SAFETY: native teardown lends the still-live server state.
    let state = unsafe { &*state };
    let mut commands = state
        .commands
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let value = if callbacks {
        commands.callbacks.pop_first().map(|(_, value)| value)
    } else {
        // Metadata stays live through native callable destruction, matching the
        // original command-map lifetime after the handler map's destructor.
        commands.handlers.pop_first().map(|(_, handler)| handler)
    };
    core::ptr::with_exposed_provenance_mut(value.unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_keep_binary_names_and_reject_duplicates_without_taking_custody() {
        let state = ServerState::new();
        let name = b"admin\0command";
        let handler = core::ptr::without_provenance_mut::<c_void>(13);
        unsafe {
            assert!(swr_v1_server_command_add(
                &raw const state,
                name.as_ptr(),
                name.len(),
                7,
                62,
                handler
            ));
            assert!(!swr_v1_server_command_add(
                &raw const state,
                name.as_ptr(),
                name.len(),
                8,
                2,
                core::ptr::without_provenance_mut(99)
            ));
            let mut info = CommandInfo::default();
            assert!(swr_v1_server_command_find(
                &raw const state,
                name.as_ptr(),
                name.len(),
                &raw mut info
            ));
            assert_eq!((info.id, info.accepted_types), (7, 62));
            assert_eq!(core::slice::from_raw_parts(info.name, info.length), name);
            assert_eq!(swr_v1_server_command_handler(&raw const state, 7), handler);
            assert!(swr_v1_server_command_handler(&raw const state, 8).is_null());
            assert_eq!(
                swr_v1_server_command_drain(&raw const state, false),
                handler
            );
            assert!(swr_v1_server_command_drain(&raw const state, false).is_null());
        }
    }

    #[test]
    fn reply_lookup_releases_the_lock_and_take_transfers_custody_exactly_once() {
        let state = ServerState::new();
        let callback = core::ptr::without_provenance_mut::<c_void>(17);
        unsafe {
            let id = swr_v1_server_command_request(&raw const state, callback);
            assert_eq!(id, 1);
            assert_eq!(
                swr_v1_server_command_callback(&raw const state, id, false),
                callback
            );
            assert!(
                state.commands.try_lock().is_ok(),
                "native callbacks may reenter the registry"
            );
            let another = swr_v1_server_command_request(
                &raw const state,
                core::ptr::without_provenance_mut(23),
            );
            assert_eq!(another, 2);
            assert_eq!(
                swr_v1_server_command_callback(&raw const state, id, true),
                callback
            );
            assert!(swr_v1_server_command_callback(&raw const state, id, true).is_null());
            assert_eq!(
                swr_v1_server_command_drain(&raw const state, true).addr(),
                23
            );
            assert!(swr_v1_server_command_drain(&raw const state, true).is_null());
        }
    }

    #[test]
    fn enumeration_owns_stable_name_bytes_across_insertions_and_request_wrap() {
        let state = ServerState::new();
        unsafe {
            assert!(swr_v1_server_command_add(
                &raw const state,
                b"a".as_ptr(),
                1,
                1,
                2,
                core::ptr::without_provenance_mut(3)
            ));
            let mut info = CommandInfo::default();
            assert!(swr_v1_server_command_at(&raw const state, 0, &raw mut info));
            let pointer = info.name;
            assert!(swr_v1_server_command_add(
                &raw const state,
                b"b".as_ptr(),
                1,
                2,
                4,
                core::ptr::without_provenance_mut(5)
            ));
            assert_eq!(*pointer, b'a');
            assert!(!swr_v1_server_command_at(
                &raw const state,
                2,
                &raw mut info
            ));
            state
                .commands
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .next_request = i64::MAX;
            assert_eq!(
                swr_v1_server_command_request(
                    &raw const state,
                    core::ptr::without_provenance_mut(7)
                ),
                i64::MAX
            );
            assert_eq!(
                swr_v1_server_command_request(
                    &raw const state,
                    core::ptr::without_provenance_mut(9)
                ),
                i64::MIN
            );
        }
    }
    #[test]
    fn metadata_precedes_native_callable_assignment_without_empty_drain_entries() {
        let state = ServerState::new();
        unsafe {
            assert!(swr_v1_server_command_add(
                &raw const state,
                b"pending".as_ptr(),
                7,
                1,
                62,
                core::ptr::null_mut()
            ));
            let mut info = CommandInfo::default();
            assert!(swr_v1_server_command_find(
                &raw const state,
                b"pending".as_ptr(),
                7,
                &raw mut info
            ));
            assert!(swr_v1_server_command_handler(&raw const state, 1).is_null());
            assert!(swr_v1_server_command_drain(&raw const state, false).is_null());
            swr_v1_server_command_attach(
                &raw const state,
                1,
                core::ptr::without_provenance_mut(31),
            );
            assert_eq!(
                swr_v1_server_command_handler(&raw const state, 1).addr(),
                31
            );
            assert!(
                state.commands.try_lock().is_ok(),
                "native copying assignment may reenter the published registry"
            );
            assert_eq!(
                swr_v1_server_command_drain(&raw const state, false).addr(),
                31
            );
            assert!(swr_v1_server_command_drain(&raw const state, false).is_null());
        }
    }
}
