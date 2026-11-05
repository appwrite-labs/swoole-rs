//! Per-thread core records. Native `ThreadGlobal` is a borrowed field view; value
//! copies have independent scalar slots and retain the original shallow pointers.
use std::{ffi::c_void, ptr};
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct CoreThreadFields {
    pub id: u16,
    pub kind: u8,
    pub main_thread: bool,
    pub buffer_stack: *mut c_void,
    pub reactor: *mut c_void,
    pub timer: *mut c_void,
    pub message_bus: *mut c_void,
    pub async_threads: *mut c_void,
    pub iouring: *mut c_void,
    pub signal_blocking_all: bool,
}
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_core_thread_new() -> *mut CoreThreadFields {
    Box::into_raw(Box::default())
}
/// # Safety
/// record is released once after all native field references end.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_core_thread_free(record: *mut CoreThreadFields) {
    if !record.is_null() {
        // SAFETY: the caller returns the unique allocation after its views end.
        unsafe { drop(Box::from_raw(record)) };
    }
}
/// # Safety
/// Both records are live; destination is exclusive and source stable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_core_thread_copy(
    destination: *mut CoreThreadFields,
    source: *const CoreThreadFields,
) {
    if !ptr::eq(destination.cast_const(), source) {
        // SAFETY: the caller lends the source for this leaf scalar/pointer snapshot.
        let fields = unsafe { *source };
        // SAFETY: destination is exclusive and initialized by this value copy.
        unsafe { *destination = fields };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn thread_snapshots_copy_values_without_sharing_scalar_slots() {
        unsafe {
            let source = swr_v1_core_thread_new();
            let copy = swr_v1_core_thread_new();
            (*source).id = 42;
            (*source).kind = 3;
            let mut token = 9u8;
            (*source).timer = ptr::from_mut(&mut token).cast();
            swr_v1_core_thread_copy(copy, source);
            assert_eq!(((*copy).id, (*copy).kind), (42, 3));
            assert_eq!((*copy).timer, (*source).timer);
            (*copy).id = 71;
            assert_eq!((*source).id, 42);
            swr_v1_core_thread_free(source);
            assert_eq!(
                ((*copy).id, (*copy).timer),
                (71, ptr::from_mut(&mut token).cast())
            );
            swr_v1_core_thread_free(copy);
        }
    }
}
