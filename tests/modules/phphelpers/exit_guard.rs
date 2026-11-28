//! Test-only owner: linked into a PHP SDK witness and calling the production
//! native trap. It carries no duplicate Zend shim or guard implementation.
use std::ffi::{c_int, c_void};

unsafe extern "C" {
    fn swr_v1_zend_try(body: unsafe extern "C" fn(*mut c_void), argument: *mut c_void) -> c_int;
}

struct Owner {
    bytes: Vec<u8>,
    dropped: unsafe extern "C" fn(*const u8, usize),
}

impl Drop for Owner {
    fn drop(&mut self) {
        // The witness callback only increments native atomics and cannot escape.
        unsafe { (self.dropped)(self.bytes.as_ptr(), self.bytes.len()) };
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_exit_guard_test_owner(
    body: unsafe extern "C" fn(*mut c_void),
    argument: *mut c_void,
    dropped: unsafe extern "C" fn(*const u8, usize),
) -> c_int {
    let owner = Box::new(Owner { bytes: vec![42; 4096], dropped });
    std::hint::black_box(&owner);
    let outcome = unsafe { swr_v1_zend_try(body, argument) };
    drop(owner);
    outcome
}
