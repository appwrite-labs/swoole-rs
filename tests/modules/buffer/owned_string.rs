use super::{CppString, TrustedAllocator, c_void};

#[test]
fn popped_owner_keeps_the_prefix_and_frees_each_allocation_once() {
    thread_local! { static FREES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
    extern "C" fn allocate(size: usize) -> *mut c_void {
        unsafe { libc::malloc(size) }
    }
    unsafe extern "C" fn free(data: *mut c_void) {
        FREES.set(FREES.get() + 1);
        unsafe { libc::free(data) };
    }
    static ALLOCATOR: crate::CoreAllocator = crate::CoreAllocator {
        malloc: Some(allocate),
        calloc: None,
        realloc: None,
        free: Some(free),
    };
    FREES.set(0);
    let allocator = unsafe { TrustedAllocator::new(&ALLOCATOR) };
    let mut buffer = CppString::EMPTY;
    buffer.alloc_exact(13, Some(allocator));
    buffer.append(b"prefix:tail");
    buffer.set_offset(7);
    let mut popped = buffer.pop_owned(13).unwrap();
    assert_eq!(popped.size(), 13, "the original exact allocation");
    assert_eq!(popped.view(), b"prefix:tail");
    assert_eq!(buffer.view(), b"tail");
    assert_eq!(buffer.size(), 16, "pop retains String's alignment");
    popped.truncate(7);
    assert_eq!(popped.view(), b"prefix:");
    drop(popped);
    assert_eq!(FREES.get(), 1);
    assert_eq!(
        buffer.view(),
        b"tail",
        "the retained tail has its own storage"
    );
    buffer.dispose();
    assert_eq!(FREES.get(), 2);
}
