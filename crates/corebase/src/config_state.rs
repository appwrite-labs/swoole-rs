//! Remaining core configuration: stable allocator/limit/hook slots and owned
//! task/resolver paths. Native containers and callbacks keep their existing guard.
use crate::{CoreStringView, StringSlot, swr_v1_core_string_set, swr_v1_core_string_view};
use std::{cell::UnsafeCell, ffi::c_void, ptr};

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct CoreAllocator {
    pub malloc: Option<unsafe extern "C" fn(usize) -> *mut c_void>,
    pub calloc: Option<unsafe extern "C" fn(usize, usize) -> *mut c_void>,
    pub realloc: Option<unsafe extern "C" fn(*mut c_void, usize) -> *mut c_void>,
    pub free: Option<unsafe extern "C" fn(*mut c_void)>,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct CoreConfigFields {
    pub max_sockets: u32,
    pub dns_cache_refresh_time: f64,
    pub std_allocator: CoreAllocator,
    pub hooks: [*mut c_void; crate::constants::SW_MAX_HOOK_TYPE as usize],
}
pub struct CoreConfig {
    fields: UnsafeCell<CoreConfigFields>,
    strings: [StringSlot; 2],
}
#[repr(C)]
pub struct CoreConfigPrepared {
    pub owner: *mut CoreConfig,
    pub fields: *mut CoreConfigFields,
    pub strings: [*const StringSlot; 2],
}
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_core_config_new() -> CoreConfigPrepared {
    let owner = Box::new(CoreConfig {
        fields: UnsafeCell::new(CoreConfigFields::default()),
        strings: std::array::from_fn(|_| StringSlot::default()),
    });
    CoreConfigPrepared {
        fields: owner.fields.get(),
        strings: owner.strings.each_ref().map(ptr::from_ref),
        owner: Box::into_raw(owner),
    }
}
/// # Safety
/// owner is released once after all native borrowed fields/string caches end.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_core_config_free(owner: *mut CoreConfig) {
    if !owner.is_null() {
        // SAFETY: the caller returns its unique Box after every borrowed view ends.
        unsafe { drop(Box::from_raw(owner)) };
    }
}
/// # Safety
/// owner is a live exclusive configuration; allocator slots are stable afterward.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_core_config_allocator(owner: *mut CoreConfig) {
    // SAFETY: caller lends the live configuration exclusively.
    let owner = unsafe { &*owner };
    // SAFETY: publish the same libc functions as the native standard allocator.
    unsafe {
        (*owner.fields.get()).std_allocator = CoreAllocator {
            malloc: Some(libc::malloc),
            calloc: Some(libc::calloc),
            realloc: Some(libc::realloc),
            free: Some(libc::free),
        }
    };
}
/// # Safety
/// owner is live/exclusive; limit follows the pinned session capacity.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_core_config_max_sockets(
    owner: *mut CoreConfig,
    limit: u32,
) -> bool {
    let observed = crate::swr_v1_core_max_sockets(limit);
    let value = u32::try_from(observed).unwrap_or(crate::constants::SW_MAX_SOCKETS_DEFAULT);
    // SAFETY: the stable owner remains live through this leaf publication.
    let owner = unsafe { &*owner };
    // SAFETY: no callback or other field access is active.
    unsafe { (*owner.fields.get()).max_sockets = value };
    observed >= 0
}
#[repr(C)]
pub struct CoreConfigCopy {
    pub status: i32,
    pub index: u32,
}
/// Copy fields through resolver-path assignment, before native resolver containers.
/// # Safety
/// destination is exclusive, source stable; native cache callbacks catch exceptions.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_core_config_copy_values(
    destination: *mut CoreConfig,
    source: *const CoreConfig,
) -> CoreConfigCopy {
    if ptr::eq(destination.cast_const(), source) {
        return CoreConfigCopy {
            status: 0,
            index: 0,
        };
    }
    // SAFETY: both owners remain live throughout the value copy.
    let destination = unsafe { &*destination };
    // SAFETY: caller lends stable source slots and scalars.
    let source = unsafe { &*source };
    // SAFETY: snapshot primitive values before any native cache effect.
    let fields = unsafe { *source.fields.get() };
    // SAFETY: first native member assignment is the socket limit.
    unsafe { (*destination.fields.get()).max_sockets = fields.max_sockets };
    // SAFETY: allocator function values follow, before task path assignment.
    unsafe { (*destination.fields.get()).std_allocator = fields.std_allocator };
    let slots = source.strings.iter().zip(&destination.strings);
    for (index, (from, to)) in slots.enumerate() {
        if index == 1 {
            // SAFETY: refresh time follows the task path and precedes resolver path.
            unsafe {
                (*destination.fields.get()).dns_cache_refresh_time = fields.dns_cache_refresh_time;
            };
        }
        // SAFETY: the source's stable string slot remains readable through assignment.
        let value = unsafe { swr_v1_core_string_view(from) };
        // SAFETY: exclusive destination; its native cache catches exceptions before publication.
        let status = unsafe { swr_v1_core_string_set(to, value.data, value.length) };
        if status != 0 {
            return CoreConfigCopy {
                status,
                index: u32::try_from(index).unwrap_or(0),
            };
        }
    }
    CoreConfigCopy {
        status: 0,
        index: 0,
    }
}
/// Copy hook pointers only after native resolver-container assignment completes.
/// # Safety
/// destination/source are live and synchronized; foreign callback custody is unchanged.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_core_config_copy_hooks(
    destination: *mut CoreConfig,
    source: *const CoreConfig,
) {
    // SAFETY: both stable owners are lent through a leaf value copy.
    let destination = unsafe { &*destination };
    // SAFETY: source is unchanged through this leaf operation.
    let source = unsafe { &*source };
    // SAFETY: copy the opaque callback slots without invoking/destroying anything.
    let hooks = unsafe { (*source.fields.get()).hooks };
    // SAFETY: destination fields are exclusive through publication.
    unsafe { (*destination.fields.get()).hooks = hooks };
}
/// The exact current path without materializing a native reference cache.
/// # Safety
/// owner is live and path bytes stay unchanged while the returned span is used.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_core_config_task_path(owner: *const CoreConfig) -> CoreStringView {
    // SAFETY: caller lends the live owner and its stable slot.
    let owner = unsafe { &*owner };
    // SAFETY: the task path remains unchanged through the returned borrow.
    unsafe { swr_v1_core_string_view(ptr::from_ref(&owner.strings[0])) }
}

#[repr(C)]
pub struct CoreTaskPathResult {
    pub status: i32,
    pub mkdir_status: i32,
}
/// Validate/create and publish a task path, including publication before its
/// final size check. Native formats diagnostics only after this returns.
/// # Safety
/// owner is live/exclusive; input is a nonempty terminated span of length bytes;
/// scratch lends capacity writable bytes, disjoint from input and owner slots.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_core_config_set_task_path(
    owner: *mut CoreConfig,
    input: *const u8,
    length: usize,
    scratch: *mut u8,
    capacity: usize,
) -> CoreTaskPathResult {
    // SAFETY: the caller guarantees a nonempty terminated input.
    if unsafe { input.read() } != b'/' {
        return CoreTaskPathResult {
            status: 1,
            mkdir_status: 0,
        };
    }
    // SAFETY: the caller lends the terminated complete path for access.
    if unsafe { libc::access(input.cast(), libc::R_OK) } < 0 {
        // SAFETY: stable input and exclusive scratch follow the complete mkdir contract.
        let mkdir_status =
            unsafe { crate::swr_v1_corebase_mkdir_complete(input, length, scratch, capacity) };
        if mkdir_status != 0 {
            return CoreTaskPathResult {
                status: 2,
                mkdir_status,
            };
        }
    }
    let suffix = crate::constants::SW_TASK_TMP_FILE.as_bytes();
    let Some(size) = length.checked_add(suffix.len() + 1) else {
        return CoreTaskPathResult {
            status: -2,
            mkdir_status: 0,
        };
    };
    let mut path = Vec::new();
    if path.try_reserve_exact(size).is_err() {
        return CoreTaskPathResult {
            status: -2,
            mkdir_status: 0,
        };
    }
    // SAFETY: copy input bytes before any native reference-cache effect can mutate them.
    path.extend_from_slice(unsafe { std::slice::from_raw_parts(input, length) });
    path.push(b'/');
    path.extend_from_slice(suffix);
    // SAFETY: the live owner is shared only through synchronized interior slots.
    let owner = unsafe { &*owner };
    // SAFETY: exclusive task slot; its native cache catches exceptions before publication.
    let status = unsafe {
        swr_v1_core_string_set(
            ptr::from_ref(&owner.strings[0]),
            path.as_ptr().cast(),
            path.len(),
        )
    };
    CoreTaskPathResult {
        status: if status != 0 {
            status
        } else if path.len() >= crate::constants::SW_TASK_TMP_PATH_SIZE as usize {
            3
        } else {
            0
        },
        mkdir_status: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::{CStr, CString};
    #[test]
    fn config_slots_are_stable_and_copied_paths_are_values() {
        unsafe {
            let source = swr_v1_core_config_new();
            let copy = swr_v1_core_config_new();
            swr_v1_core_config_allocator(source.owner);
            assert!((*source.fields).std_allocator.malloc.is_some());
            assert!(swr_v1_core_config_max_sockets(source.owner, 1));
            assert_eq!((*source.fields).max_sockets, 1);
            (*source.fields).dns_cache_refresh_time = 17.5;
            assert_eq!(
                swr_v1_core_string_set(source.strings[0], c"/tmp/source".as_ptr(), 11),
                0
            );
            assert_eq!(
                swr_v1_core_string_set(source.strings[1], c"/etc/source".as_ptr(), 11),
                0
            );
            let address = ptr::addr_of_mut!((*copy.fields).std_allocator);
            assert_eq!(
                swr_v1_core_config_copy_values(copy.owner, source.owner).status,
                0
            );
            assert_eq!((*copy.fields).dns_cache_refresh_time, 17.5);
            assert_eq!(address, ptr::addr_of_mut!((*copy.fields).std_allocator));
            assert_eq!(
                swr_v1_core_string_set(source.strings[0], c"changed".as_ptr(), 7),
                0
            );
            assert_eq!(
                CStr::from_ptr(swr_v1_core_config_task_path(copy.owner).data).to_bytes(),
                b"/tmp/source"
            );
            let allocation = ((*copy.fields).std_allocator.malloc.unwrap())(19);
            assert!(!allocation.is_null());
            ((*copy.fields).std_allocator.free.unwrap())(allocation);
            swr_v1_core_config_free(source.owner);
            swr_v1_core_config_free(copy.owner);
        }
    }
    #[test]
    fn task_directory_creation_and_rejection_keep_the_published_value() {
        let root = std::env::temp_dir().join(format!("swoole-core-config-{}", std::process::id()));
        let nested = root.join("nested");
        let path = CString::new(nested.as_os_str().as_encoded_bytes()).unwrap();
        unsafe {
            let owner = swr_v1_core_config_new();
            let mut scratch = [0u8; 4096];
            assert_eq!(
                swr_v1_core_config_set_task_path(
                    owner.owner,
                    path.as_ptr().cast(),
                    path.as_bytes().len(),
                    scratch.as_mut_ptr(),
                    scratch.len()
                )
                .status,
                0
            );
            assert!(nested.is_dir());
            let expected = [
                path.as_bytes(),
                b"/",
                crate::constants::SW_TASK_TMP_FILE.as_bytes(),
            ]
            .concat();
            assert_eq!(
                CStr::from_ptr(swr_v1_core_config_task_path(owner.owner).data).to_bytes(),
                expected
            );
            assert_eq!(
                swr_v1_core_config_set_task_path(
                    owner.owner,
                    c"relative".as_ptr().cast(),
                    8,
                    scratch.as_mut_ptr(),
                    scratch.len()
                )
                .status,
                1
            );
            assert_eq!(
                CStr::from_ptr(swr_v1_core_config_task_path(owner.owner).data).to_bytes(),
                expected
            );
            let long = root
                .join("a".repeat(100))
                .join("b".repeat(100))
                .join("c".repeat(100));
            let long = CString::new(long.as_os_str().as_encoded_bytes()).unwrap();
            assert_eq!(
                swr_v1_core_config_set_task_path(
                    owner.owner,
                    long.as_ptr().cast(),
                    long.as_bytes().len(),
                    scratch.as_mut_ptr(),
                    scratch.len()
                )
                .status,
                3
            );
            assert!(swr_v1_core_config_task_path(owner.owner).length >= 256);
            swr_v1_core_config_free(owner.owner);
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
