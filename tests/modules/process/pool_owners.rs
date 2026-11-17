use super::{PoolRecord, PoolRef};
use std::rc::Rc;

#[test]
#[cfg_attr(miri, ignore = "Miri has no shared mappings or fork")]
fn borrowing_ipc_in_one_process_keeps_it_available_to_another() {
    let _children = crate::process::wait::tests::CHILDREN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let size = size_of::<PoolRecord>();
    let mapping = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    assert_ne!(mapping, libc::MAP_FAILED);
    let raw = mapping.cast::<PoolRecord>();
    unsafe { raw.write(PoolRecord::empty()) };
    unsafe { (*raw).ipc.set(super::pool_ipc_new(false)) };
    let record = unsafe { &*raw };
    let mut pipe = [-1; 2];
    assert_eq!(unsafe { libc::pipe(pipe.as_mut_ptr()) }, 0);
    let child = unsafe { libc::fork() };
    assert!(child >= 0);
    if child == 0 {
        // Start before the parent borrows, then inspect the shared record while
        // that borrow is held. The IPC allocation itself is process-private.
        unsafe { libc::alarm(5) };
        let mut byte = 0u8;
        let received = unsafe { libc::read(pipe[0], (&raw mut byte).cast(), 1) };
        let available = received == 1 && record.with_ipc(|_| ()).is_some();
        unsafe { libc::_exit(i32::from(!available)) };
    }
    let mut status = 0;
    let result = record.with_ipc(|_| {
        assert_eq!(unsafe { libc::write(pipe[1], b"x".as_ptr().cast(), 1) }, 1);
        loop {
            let waited = unsafe { libc::waitpid(child, &raw mut status, 0) };
            if waited != -1 || std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
                return waited;
            }
        }
    });
    unsafe {
        libc::close(pipe[0]);
        libc::close(pipe[1]);
        drop(Box::from_raw(record.ipc.get()));
        std::ptr::drop_in_place(raw);
        libc::munmap(mapping, size);
    }
    assert_eq!(result, Some(child));
    assert!(libc::WIFEXITED(status));
    assert_eq!(
        libc::WEXITSTATUS(status),
        0,
        "another process must still find IPC"
    );
}

#[test]
fn a_borrowed_bus_cannot_be_freed_by_a_reentrant_operation() {
    let mut record = PoolRecord::empty();
    let pool = PoolRef::from(&mut record);
    let owner = Rc::new(());
    pool.set_message_bus(Some(Box::new(owner.clone())));
    assert_eq!(Rc::strong_count(&owner), 2);
    pool.with_message_bus(|bus| {
        assert!(bus.is::<Rc<()>>());
        // The lent bus is out of the pool: a reentrant operation finds none to free.
        assert!(!pool.has_message_bus());
        pool.set_message_bus(None);
        assert_eq!(Rc::strong_count(&owner), 2);
    });
    assert!(pool.has_message_bus());
    assert_eq!(Rc::strong_count(&owner), 2);
    pool.set_message_bus(None);
    assert!(!pool.has_message_bus());
    assert_eq!(Rc::strong_count(&owner), 1);
}
