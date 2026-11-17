use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use swoole_rs_bus::Bus;
use swoole_rs_corebase::{CoreAllocator, TrustedAllocator};

static ALLOCATOR: CoreAllocator = CoreAllocator {
    malloc: Some(swoole_rs_corebase::sys::malloc),
    calloc: Some(swoole_rs_corebase::sys::calloc),
    realloc: Some(libc::realloc),
    free: Some(libc::free),
};

#[test]
fn receive_storage_uses_its_allocator_on_replace_failure_and_drop() {
    static FREES: AtomicU64 = AtomicU64::new(0);
    unsafe extern "C" fn free(data: *mut c_void) {
        FREES.fetch_add(1, Ordering::Relaxed);
        unsafe { libc::free(data) };
    }
    static OWNED: CoreAllocator = CoreAllocator {
        free: Some(free),
        ..ALLOCATOR
    };
    static EMPTY_ALLOCATOR: CoreAllocator = CoreAllocator {
        malloc: None,
        calloc: None,
        realloc: None,
        free: None,
    };
    let owned = unsafe { TrustedAllocator::new(&OWNED) };
    let empty = unsafe { TrustedAllocator::new(&EMPTY_ALLOCATOR) };
    let bus = Bus::create(128);
    assert!(bus.alloc_buffer(128, owned));
    assert!(bus.alloc_buffer(64, owned));
    assert_eq!(FREES.load(Ordering::Relaxed), 1);
    assert!(!bus.alloc_buffer(128, empty));
    assert_eq!(FREES.load(Ordering::Relaxed), 2);
    assert!(!bus.holds_whole());
    assert!(bus.alloc_buffer(128, owned));
    drop(bus);
    assert_eq!(FREES.load(Ordering::Relaxed), 3);
}
