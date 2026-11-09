//! Server static configuration, shared with contexts using its original identity.
//! Native views never retain an independent owning collection.
use crate::ServerState;
use core::ffi::c_void;
use std::sync::{Arc, Mutex, MutexGuard};
use swoole_rs_http::{HttpStaticContext, RewriteRules};

#[derive(Default)]
struct Strings {
    values: Vec<Vec<u8>>,
    enabled: bool,
}
pub(crate) struct StaticConfig {
    tables: [Arc<Mutex<Strings>>; 3],
    rewrite_rules: RewriteRules,
}
impl StaticConfig {
    fn new() -> Self {
        Self {
            tables: std::array::from_fn(|_| Arc::default()),
            rewrite_rules: RewriteRules::default(),
        }
    }
}
fn owner(state: &ServerState) -> MutexGuard<'_, StaticConfig> {
    state
        .static_config
        .get_or_init(|| Mutex::new(StaticConfig::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
fn table(state: &ServerState, kind: usize) -> Arc<Mutex<Strings>> {
    owner(state)
        .tables
        .get(kind)
        .map(Arc::clone)
        .unwrap_or_default()
}

/// Collection `kind`: whether it is set, and a copy of its values.
pub(crate) fn strings(state: &ServerState, kind: u32) -> (bool, Vec<Vec<u8>>) {
    let table = table(state, usize::try_from(kind).unwrap_or(usize::MAX));
    let strings = table
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    (strings.enabled, strings.values.clone())
}

/// Share the current collection identity. Replacing the Server's optional
/// collection leaves already-bound contexts holding the original table.
/// # Safety
/// `state` is live; the result needs one `_owner_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_server_static_owner(
    state: *const ServerState,
    kind: u32,
) -> *const c_void {
    // SAFETY: the caller lends a live Server state.
    let state = unsafe { &*state };
    let Ok(kind) = usize::try_from(kind) else {
        return core::ptr::null();
    };
    if kind >= 3 {
        return core::ptr::null();
    }
    Arc::into_raw(table(state, kind)).cast()
}
/// # Safety
/// `handle` is one live strong reference from `_static_owner`/clone.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_server_static_owner_clone(handle: *const c_void) -> *const c_void {
    // SAFETY: the raw Arc retains at least one strong reference.
    unsafe {
        Arc::increment_strong_count(handle.cast::<Mutex<Strings>>());
    }
    handle
}
/// # Safety
/// `handle` is null or one unreleased raw strong reference.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_server_static_owner_free(handle: *const c_void) {
    if !handle.is_null() {
        // SAFETY: the caller transfers exactly one raw strong reference back.
        drop(unsafe { Arc::from_raw(handle.cast::<Mutex<Strings>>()) });
    }
}
/// MIME membership: null input queries whether the optional set exists.
/// # Safety
/// `handle` is live; non-null `data` lends `length` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_server_static_mime(
    handle: *const c_void,
    data: *const u8,
    length: usize,
) -> bool {
    // SAFETY: the native context retains its raw Arc throughout this leaf call.
    let strings = unsafe { &*handle.cast::<Mutex<Strings>>() };
    let strings = strings
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if data.is_null() {
        return strings.enabled;
    }
    // SAFETY: the caller lends exactly this byte range.
    let bytes = unsafe { core::slice::from_raw_parts(data, length) };
    strings.values.iter().any(|value| value == bytes)
}

/// 0 count, 1 enabled, 2 enable, 3 replace with absent, 4 insert unique,
/// 5 find (count when absent), 6 append preserving duplicates. The named index
/// setter does its original duplicate lookup before appending.
/// # Safety
/// `state` is live; insertion/search lends `length` readable bytes (non-null even
/// when empty). Writes use the original configuration's synchronization.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_server_static_config(
    state: *const ServerState,
    kind: u32,
    operation: u32,
    data: *const u8,
    length: usize,
) -> usize {
    let Ok(kind) = usize::try_from(kind) else {
        return 0;
    };
    if kind >= 3 {
        return 0;
    }
    // SAFETY: the caller lends a live Server state.
    let state = unsafe { &*state };
    if operation == 3 {
        // Assignment replaces the shared_ptr identity; contexts holding the old
        // table continue to observe it, rather than a copy or the new collection.
        if let Some(table) = owner(state).tables.get_mut(kind) {
            *table = Arc::default();
        }
        return 0;
    }
    let table = table(state, kind);
    let mut strings = table
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match operation {
        0 => strings.values.len(),
        1 => usize::from(strings.enabled),
        2 => {
            strings.enabled = true;
            0
        }
        4..=6 => {
            // SAFETY: these operations lend the specified input range.
            let bytes = unsafe { core::slice::from_raw_parts(data, length) };
            let found = strings.values.iter().position(|value| value == bytes);
            if operation == 4 || operation == 6 {
                strings.enabled = true;
                if operation == 6 || found.is_none() {
                    strings.values.push(bytes.to_vec());
                }
            }
            found.unwrap_or(strings.values.len())
        }
        _ => 0,
    }
}
/// # Safety
/// `state` is live and `length` writable. Bytes remain borrowed until collection
/// replacement or server destruction; no replacement races a native reader.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_server_static_text(
    state: *const ServerState,
    kind: u32,
    index: usize,
    length: *mut usize,
) -> *const u8 {
    // SAFETY: the caller lends a live Server state.
    let state = unsafe { &*state };
    let Ok(kind) = usize::try_from(kind) else {
        return core::ptr::null();
    };
    if kind >= 3 {
        return core::ptr::null();
    }
    let table = table(state, kind);
    let strings = table
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let value = strings.values.get(index);
    // SAFETY: the caller supplies writable output length.
    unsafe {
        length.write(value.map_or(0, Vec::len));
    }
    value.map_or(core::ptr::null(), Vec::as_ptr)
}

/// `url_rewrite_rules`: add one rule (upstream's `Server::add_rewrite_rule`).
pub(crate) fn add_rewrite_rule(state: &ServerState, pattern: &[u8], replacement: &[u8]) {
    owner(state).rewrite_rules.add(pattern, replacement);
}
/// # Safety
/// `state` is live; each span lends `length` readable bytes (null only when empty).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_server_static_rewrite_add(
    state: *const ServerState,
    pattern: *const u8,
    pattern_length: usize,
    replacement: *const u8,
    replacement_length: usize,
) {
    let span = |data: *const u8, length| {
        if length == 0 {
            &[][..]
        } else {
            // SAFETY: the caller lends `length` readable bytes at `data`.
            unsafe { core::slice::from_raw_parts(data, length) }
        }
    };
    // SAFETY: the caller lends a live Server state.
    let state = unsafe { &*state };
    add_rewrite_rule(
        state,
        span(pattern, pattern_length),
        span(replacement, replacement_length),
    );
}
/// Upstream's `Server::apply_rewrite_rules`: rewrite the static context's URL by the
/// server's rules. True when a rule applied.
/// # Safety
/// `state` is live and `context` is a live static context no one else uses meanwhile.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_server_static_rewrite(
    state: *const ServerState,
    context: *mut HttpStaticContext,
) -> bool {
    // SAFETY: the caller lends a live Server state.
    let state = unsafe { &*state };
    // SAFETY: the caller lends the context exclusively.
    let context = unsafe { &mut *context };
    apply_rewrite_rules(state, context)
}
/// Upstream's `Server::apply_rewrite_rules` over the server's rules.
pub(crate) fn apply_rewrite_rules(state: &ServerState, context: &mut HttpStaticContext) -> bool {
    context.rewrite(&owner(state).rewrite_rules)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn optional_collections_distinguish_absent_empty_and_ordered_unique_values() {
        let state = ServerState::new();
        unsafe {
            let input = |kind, op, bytes: &[u8]| {
                swr_v1_server_static_config(&raw const state, kind, op, bytes.as_ptr(), bytes.len())
            };
            assert_eq!(input(0, 1, b""), 0);
            input(0, 2, b"");
            assert_eq!(input(0, 1, b""), 1);
            assert_eq!(input(0, 0, b""), 0);
            input(1, 4, b"index.php");
            input(1, 4, b"index.html");
            input(1, 4, b"index.php");
            assert_eq!(input(1, 0, b""), 2);
            assert_eq!(input(1, 5, b"index.php"), 0);
            assert_eq!(input(1, 5, b"missing"), 2);
            let mut length = 0;
            let text = swr_v1_server_static_text(&raw const state, 1, 1, &raw mut length);
            assert_eq!(core::slice::from_raw_parts(text, length), b"index.html");
            input(2, 4, b"text/html");
            input(2, 4, b"Text/Html");
            assert_eq!(
                input(2, 0, b""),
                2,
                "MIME configuration preserves upstream's case-sensitive byte keys"
            );
            assert_eq!(input(0, 0, b""), 0, "each collection owns separate state");
            input(1, 6, b"index.php");
            assert_eq!(
                input(1, 0, b""),
                3,
                "direct public vector assignment/appends preserve duplicates"
            );
            input(1, 3, b"");
            assert_eq!(input(1, 1, b""), 0);
            assert_eq!(input(1, 0, b""), 0);
            assert!(swr_v1_server_static_text(&raw const state, 1, 0, &raw mut length).is_null());
        }
    }
    #[test]
    fn rewrite_rules_rewrite_the_context_url() {
        let state = ServerState::new();
        let mut context = HttpStaticContext::new(b"/view/post/3", 128).unwrap();
        assert!(!apply_rewrite_rules(&state, &mut context));
        add_rewrite_rule(&state, b"~^/view/post/(\\d+)$~", b"/static/$1.html");
        add_rewrite_rule(&state, b"/article/", b"/static/article/");
        assert!(apply_rewrite_rules(&state, &mut context));
        assert!(
            !apply_rewrite_rules(&state, &mut context),
            "/static/3.html matches neither"
        );
    }
    #[test]
    fn shared_context_owner_observes_mutation_and_outlives_native_server() {
        let state = Box::new(ServerState::new());
        unsafe {
            let handle = swr_v1_server_static_owner(&raw const *state, 2);
            assert!(!swr_v1_server_static_mime(handle, core::ptr::null(), 0));
            swr_v1_server_static_config(&raw const *state, 2, 4, b"text/html".as_ptr(), 9);
            assert!(swr_v1_server_static_mime(handle, core::ptr::null(), 0));
            assert!(swr_v1_server_static_mime(handle, b"text/html".as_ptr(), 9));
            let copy = swr_v1_server_static_owner_clone(handle);
            drop(state);
            swr_v1_server_static_owner_free(handle);
            assert!(swr_v1_server_static_mime(copy, b"text/html".as_ptr(), 9));
            assert!(!swr_v1_server_static_mime(copy, b"image/png".as_ptr(), 9));
            swr_v1_server_static_owner_free(copy);
        }
    }
}
