//! Static file locations, ordered index names and compression MIME types.
//! The configuration is published before workers start. Native iterators are
//! borrowed views; they never retain an independent owning collection.
use crate::ServerState;

pub(crate) struct StaticConfig {
    values: [Vec<Vec<u8>>; 3],
    enabled: [bool; 3],
}
impl StaticConfig {
    pub(crate) const fn new() -> Self {
        Self {
            values: [Vec::new(), Vec::new(), Vec::new()],
            enabled: [false; 3],
        }
    }
}

/// Query or alter a collection: 0 count, 1 enabled, 2 enable, 3 clear/disable,
/// 4 insert unique bytes, 5 find (count when absent). Index names retain order.
/// # Safety
/// `state` is live; insertion/search lends `length` readable bytes (a non-null
/// pointer even when empty). Mutation occurs before worker publication.
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
    // SAFETY: the caller lends the live server's configuration owner.
    let state = unsafe { &*state };
    let mut config = state
        .static_config
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match operation {
        0 => config.values[kind].len(),
        1 => usize::from(config.enabled[kind]),
        2 => {
            config.enabled[kind] = true;
            0
        }
        3 => {
            config.values[kind].clear();
            config.enabled[kind] = false;
            0
        }
        4 | 5 => {
            // SAFETY: these two operations lend the specified readable byte range.
            let bytes = unsafe { core::slice::from_raw_parts(data, length) };
            let found = config.values[kind].iter().position(|value| value == bytes);
            if operation == 4 {
                config.enabled[kind] = true;
                if found.is_none() {
                    config.values[kind].push(bytes.to_vec());
                }
            }
            found.unwrap_or(config.values[kind].len())
        }
        _ => 0,
    }
}

/// # Safety
/// `state` is live and `length` writable. The returned bytes are a borrowed
/// immutable view, valid until configuration clear/server destruction. Mutations
/// happen before workers start and no clear races a published reader.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_server_static_text(
    state: *const ServerState,
    kind: u32,
    index: usize,
    length: *mut usize,
) -> *const u8 {
    // SAFETY: the caller lends a live server state.
    let state = unsafe { &*state };
    let config = state
        .static_config
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let value = config
        .values
        .get(kind as usize)
        .and_then(|values| values.get(index));
    // SAFETY: the caller supplies writable output length.
    unsafe {
        length.write(value.map_or(0, Vec::len));
    }
    value.map_or(core::ptr::null(), Vec::as_ptr)
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
            input(1, 3, b"");
            assert_eq!(input(1, 1, b""), 0);
            assert_eq!(input(1, 0, b""), 0);
            assert!(swr_v1_server_static_text(&raw const state, 1, 0, &raw mut length).is_null());
        }
    }
}
