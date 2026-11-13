//! Server static configuration, shared with contexts using its original identity.
//! Native views never retain an independent owning collection.
use crate::ServerState;
use core::ffi::c_void;
use std::sync::{Arc, Mutex, MutexGuard};
use swoole_rs_http::{HttpStaticContext, RewriteRules};

#[derive(Default)]
struct Strings {
    /// Copy-on-write, so a request reads them without holding the lock, and each
    /// value's bytes stay where `_static_text` lent them.
    values: Arc<Vec<Arc<[u8]>>>,
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

/// Collection `kind`: whether it is set, and its values, read after the lock is released.
pub(crate) fn strings<R>(
    state: &ServerState,
    kind: u32,
    f: impl FnOnce(bool, &[Arc<[u8]>]) -> R,
) -> R {
    let table = table(state, usize::try_from(kind).unwrap_or(usize::MAX));
    let (enabled, values) = {
        let strings = table
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (strings.enabled, Arc::clone(&strings.values))
    };
    f(enabled, &values)
}

/// Share the current collection identity. Replacing the Server's optional
/// collection leaves already-bound contexts holding the original table.
/// The result needs one `_owner_free`.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_server_static_owner(state: &ServerState, kind: u32) -> *const c_void {
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
    strings.values.iter().any(|value| **value == *bytes)
}

/// [`config`] for native code.
/// # Safety
/// Insertion/search (4 to 6) lends `length` readable bytes (non-null even when
/// empty).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_server_static_config(
    state: &ServerState,
    kind: u32,
    operation: u32,
    data: *const u8,
    length: usize,
) -> usize {
    let bytes = if (4..=6).contains(&operation) {
        // SAFETY: these operations lend the specified input range.
        unsafe { core::slice::from_raw_parts(data, length) }
    } else {
        &[]
    };
    config(state, kind, operation, bytes)
}

/// Operation `operation` on the static collection `kind`: 0 count, 1 enabled,
/// 2 enable, 3 replace with absent, 4 insert `bytes` unique, 5 find `bytes`
/// (count when absent), 6 append `bytes` preserving duplicates. The named index
/// setter does its original duplicate lookup before appending.
pub(crate) fn config(state: &ServerState, kind: u32, operation: u32, bytes: &[u8]) -> usize {
    let Ok(kind) = usize::try_from(kind) else {
        return 0;
    };
    if kind >= 3 {
        return 0;
    }
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
            let found = strings.values.iter().position(|value| **value == *bytes);
            if operation == 4 || operation == 6 {
                strings.enabled = true;
                if operation == 6 || found.is_none() {
                    Arc::make_mut(&mut strings.values).push(Arc::from(bytes));
                }
            }
            found.unwrap_or(strings.values.len())
        }
        _ => 0,
    }
}
/// Bytes remain borrowed until collection replacement or server destruction; no
/// replacement races a native reader.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_server_static_text(
    state: &ServerState,
    kind: u32,
    index: usize,
    length: &mut usize,
) -> *const u8 {
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
    *length = value.map_or(0, |value| value.len());
    value.map_or(core::ptr::null(), |value| value.as_ptr())
}

/// `url_rewrite_rules`: add one rule (upstream's `Server::add_rewrite_rule`).
pub(crate) fn add_rewrite_rule(state: &ServerState, pattern: &[u8], replacement: &[u8]) {
    owner(state).rewrite_rules.add(pattern, replacement);
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
                swr_v1_server_static_config(&state, kind, op, bytes.as_ptr(), bytes.len())
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
            let text = swr_v1_server_static_text(&state, 1, 1, &mut length);
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
            assert!(swr_v1_server_static_text(&state, 1, 0, &mut length).is_null());
        }
    }
    #[test]
    fn a_writer_proceeds_while_a_reader_holds_the_snapshot_and_lent_bytes_survive() {
        // Freed only after the writer joins: one stuck on the lock fails the test first and
        // leaves the state leaked, since that writer would outlive the test's frame.
        let owned = Box::into_raw(Box::new(ServerState::new()));
        let state: &'static ServerState = unsafe { &*owned };
        let address = std::ptr::from_ref(state) as usize;
        let append = move |bytes: &'static [u8]| unsafe {
            swr_v1_server_static_config(
                &*(address as *const ServerState),
                1,
                6,
                bytes.as_ptr(),
                bytes.len(),
            )
        };
        append(b"index.php");
        let mut length = 0;
        let lent = swr_v1_server_static_text(state, 1, 0, &mut length);
        let writer = strings(state, 1, |enabled, values| {
            assert!(enabled);
            let (done, wait) = std::sync::mpsc::channel();
            let writer = std::thread::spawn(move || done.send(append(b"index.html")));
            assert_eq!(
                wait.recv_timeout(std::time::Duration::from_secs(10)),
                Ok(2),
                "the append doesn't wait for the reader"
            );
            assert_eq!(values.len(), 1, "the reader keeps its snapshot");
            writer
        });
        assert_eq!(append(b"default.htm"), 3);
        assert_eq!(
            unsafe { core::slice::from_raw_parts(lent, length) },
            b"index.php"
        );
        assert!(writer.join().is_ok_and(|sent| sent.is_ok()));
        drop(unsafe { Box::from_raw(owned) });
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
            let handle = swr_v1_server_static_owner(&state, 2);
            assert!(!swr_v1_server_static_mime(handle, core::ptr::null(), 0));
            swr_v1_server_static_config(&state, 2, 4, b"text/html".as_ptr(), 9);
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
    // Ported from tests/modules/http/server_config_owners.cc, which drove these calls
    // through the C++ ServerStrings/ServerMimeTypes views.
    #[test]
    fn bound_owners_keep_their_table_across_replacement_and_server_free() {
        let state = Box::new(ServerState::new());
        unsafe {
            let config = |kind, op, bytes: &[u8]| {
                swr_v1_server_static_config(&state, kind, op, bytes.as_ptr(), bytes.len())
            };
            let text = |kind, index| {
                let mut length = 0;
                let data = swr_v1_server_static_text(&state, kind, index, &mut length);
                core::slice::from_raw_parts(data, length).to_vec()
            };
            let has = |handle, bytes: &[u8]| {
                swr_v1_server_static_mime(handle, bytes.as_ptr(), bytes.len())
            };
            // Locations: absent until the first insert, which is unique.
            assert_eq!(config(0, 1, b""), 0);
            config(0, 4, b"/");
            config(0, 4, b"/");
            assert_eq!(config(0, 1, b""), 1);
            assert_eq!(config(0, 0, b""), 1);
            // Index files: assignment replaces, enables and appends with duplicates.
            config(1, 3, b"");
            config(1, 2, b"");
            for name in [&b"a"[..], b"a", b"b"] {
                config(1, 6, name);
            }
            assert_eq!(config(1, 0, b""), 3);
            assert_eq!(
                (text(1, 0), text(1, 1), text(1, 2)),
                (b"a".to_vec(), b"a".to_vec(), b"b".to_vec())
            );
            config(1, 6, b"b");
            assert_eq!(config(1, 0, b""), 4);
            assert!(config(1, 5, b"b") < config(1, 0, b""));
            // MIME types: a bound owner sees later inserts but not a replacement.
            assert_eq!(config(2, 1, b""), 0, "an absent set binds no owner");
            config(2, 4, b"text/html");
            let first = swr_v1_server_static_owner(&state, 2);
            config(2, 4, b"image/png");
            assert!(swr_v1_server_static_mime(first, core::ptr::null(), 0));
            assert!(has(first, b"image/png"));
            config(2, 3, b"");
            config(2, 2, b"");
            config(2, 6, b"text/plain");
            assert!(has(first, b"image/png"));
            assert!(!has(first, b"text/plain"));
            let second = swr_v1_server_static_owner(&state, 2);
            assert!(has(second, b"text/plain"));
            let copy = swr_v1_server_static_owner_clone(first);
            swr_v1_server_static_owner_free(copy);
            let copy = swr_v1_server_static_owner_clone(second);
            drop(state);
            assert!(has(first, b"text/html"));
            assert!(has(second, b"text/plain"));
            assert!(has(copy, b"text/plain"));
            for handle in [first, second, copy] {
                swr_v1_server_static_owner_free(handle);
            }
        }
    }
}
