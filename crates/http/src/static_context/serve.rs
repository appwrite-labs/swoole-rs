//! Selected filesystem-path policy; Native returns copied observations after effects.
use super::{FILENAME, HttpProxyText, HttpStaticContext, INVALID, bytes};
use crate::static_meta::static_catch;
use crate::static_path as path;

#[derive(Default)]
pub(super) struct Serve {
    facts: u32,
    pending: i32,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct HttpContextServeObservation {
    pub event: u32,
    pub observed: u32,
    pub has_locations: u32,
    pub has_index: u32,
    pub autoindex: u32,
    pub value: HttpProxyText,
}
/// Events begin0/config, locations-finished1, stat2, canonical3, MIME4.
/// Stat metadata is admitted by `observed()` only on Native success. Canonical
/// bytes are copied before return; no Native path/configuration pointer is stored.
/// Returns existing path actions, with directory publication and terminal error
/// handling completed in Rust. Legacy `path()` and v1 contracts remain unchanged.
/// # Safety
/// Unique live owner; initialized, independent root/observation spans. Native
/// effects complete before this call, never through a Rust frame/borrow.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v2_http_context_serve(
    owner: *mut HttpStaticContext,
    root: *const HttpProxyText,
    observation: *const HttpContextServeObservation,
) -> i32 {
    // SAFETY: `owner` is null or live, aligned and exclusively lent for this call.
    let Some(owner) = (unsafe { owner.as_mut() }) else {
        return INVALID;
    };
    // SAFETY: `root` is null or a live, aligned text descriptor.
    let root = unsafe { root.as_ref() };
    // SAFETY: `observation` is null or a live, aligned observation.
    let observation = unsafe { observation.as_ref() };
    let (Some(root), Some(observation)) = (root, observation) else {
        return INVALID;
    };
    // SAFETY: the root descriptor names `length` initialized bytes.
    let root = match unsafe { bytes(root.data, root.length) } {
        Ok(root) => root,
        Err(error) => return error,
    };
    match observation.event {
        0 => {
            owner.path.phase = 0;
            owner.path.url_length = 0;
            owner.serve.facts = if observation.has_locations != 0 {
                path::LOCATIONS
            } else {
                0
            } | if observation.has_index != 0 {
                path::INDEX
            } else {
                0
            } | if observation.autoindex != 0 {
                path::AUTOINDEX
            } else {
                0
            };
        }
        1 if owner.serve.pending == 2 => {}
        2 if owner.serve.pending == 4 => {
            owner.serve.facts &= !(path::STAT_OK | path::LINK | path::DIRECTORY | path::FILE);
            if observation.observed != 0 {
                owner.serve.facts |= path::STAT_OK | owner.facts.kind;
            }
        }
        3 if owner.serve.pending == 5 => {
            owner.serve.facts &= !path::CANONICAL_OK;
            if observation.observed != 0 {
                // SAFETY: an observed value names `length` initialized bytes.
                let value = match unsafe { bytes(observation.value.data, observation.value.length) }
                {
                    Ok(value) => value,
                    Err(error) => return error,
                };
                if value.len() < owner.filename.len() {
                    let status = owner.publish(FILENAME, value);
                    if status < 0 {
                        return status;
                    }
                    owner.serve.facts |= path::CANONICAL_OK;
                }
            }
        }
        4 if owner.serve.pending == 6 => {
            owner.serve.facts &= !path::MIME;
            if observation.observed != 0 {
                owner.serve.facts |= path::MIME;
            }
        }
        _ => return INVALID,
    }
    loop {
        let action = owner.path_step(root, owner.serve.facts);
        match action {
            3 => {} // Directory bytes were published by the owned path step.
            7 => {
                let result = static_catch(owner.path.last, &mut owner.facts.status);
                owner.serve.pending = result;
                return result;
            }
            _ => {
                owner.serve.pending = action;
                return action;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        HttpStaticObservation, swr_v2_http_context_drop, swr_v2_http_context_new,
        swr_v2_http_context_observed, swr_v2_http_context_path,
    };
    use std::ptr;
    fn observation(event: u32) -> HttpContextServeObservation {
        HttpContextServeObservation {
            event,
            observed: 1,
            has_locations: 0,
            has_index: 0,
            autoindex: 0,
            value: HttpProxyText {
                data: ptr::null(),
                length: 0,
            },
        }
    }
    fn step(owner: *mut HttpStaticContext, event: u32) -> i32 {
        let root = HttpProxyText {
            data: b"/root".as_ptr(),
            length: 5,
        };
        unsafe { swr_v2_http_context_serve(owner, &raw const root, &observation(event)) }
    }
    fn stat(owner: *mut HttpStaticContext, kind: u32) {
        let facts = HttpStaticObservation {
            filesize: 17,
            kind,
            ..HttpStaticObservation::default()
        };
        assert_eq!(
            unsafe { swr_v2_http_context_observed(owner, 0, &raw const facts) },
            0
        );
    }
    #[test]
    fn owned_fact_flow_preserves_configured_location_and_repeated_entry() {
        let owner = unsafe { swr_v2_http_context_new(b"/file.txt".as_ptr(), 9, 4096) };
        assert!(!owner.is_null());
        let root = HttpProxyText {
            data: b"/root".as_ptr(),
            length: 5,
        };
        for _ in 0..2 {
            let mut begin = observation(0);
            begin.has_locations = 1;
            assert_eq!(
                unsafe { swr_v2_http_context_serve(owner, &raw const root, &raw const begin) },
                2
            );
            assert_eq!(
                unsafe { swr_v2_http_context_path(owner, 1, b"/".as_ptr(), 1, 0) },
                1
            );
            assert_eq!(step(owner, 1), 4);
            stat(owner, path::FILE);
            assert_eq!(step(owner, 2), 6);
            assert_eq!(step(owner, 4), 1);
            assert_eq!(unsafe { (*owner).path.last }, 1);
        }
        unsafe { swr_v2_http_context_drop(owner) };
    }
    #[test]
    fn canonical_observation_admits_owned_filename_before_following_stat() {
        let owner = unsafe { swr_v2_http_context_new(b"/alias.txt".as_ptr(), 10, 4096) };
        assert!(!owner.is_null());
        assert_eq!(step(owner, 0), 2);
        assert_eq!(step(owner, 1), 4);
        stat(owner, path::LINK);
        assert_eq!(step(owner, 2), 5);
        let mut canonical = observation(3);
        canonical.value = HttpProxyText {
            data: b"/root/file.txt".as_ptr(),
            length: 14,
        };
        let root = HttpProxyText {
            data: b"/root".as_ptr(),
            length: 5,
        };
        assert_eq!(
            unsafe { swr_v2_http_context_serve(owner, &raw const root, &raw const canonical) },
            4
        );
        assert_eq!(unsafe { &(&(*owner).filename)[..15] }, b"/root/file.txt\0");
        stat(owner, path::FILE);
        assert_eq!(step(owner, 2), 6);
        assert_eq!(step(owner, 4), 1);
        unsafe { swr_v2_http_context_drop(owner) };
    }
}
