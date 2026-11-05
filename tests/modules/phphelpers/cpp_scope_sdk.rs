//! SDK-only owner witness: no Zend imports or native transport implementation.
use std::ffi::{c_int, c_void};

struct Owner {
    bytes: Vec<u8>,
    dropped: unsafe extern "C" fn(*const u8, usize),
}
impl Drop for Owner {
    fn drop(&mut self) {
        // SAFETY: the native observer only reads these live bytes and cannot escape.
        unsafe { (self.dropped)(self.bytes.as_ptr(), self.bytes.len()) };
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cpp_scope_test_owner(
    body: unsafe extern "C" fn(*mut c_void) -> c_int,
    argument: *mut c_void,
    dropped: unsafe extern "C" fn(*const u8, usize),
) -> c_int {
    let owner = Box::new(Owner { bytes: vec![42; 4096], dropped });
    std::hint::black_box(&owner);
    // SAFETY: native capture contains C++ exceptions before returning through Rust.
    let outcome = unsafe { body(argument) };
    drop(owner);
    outcome
}
