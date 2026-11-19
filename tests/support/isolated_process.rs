include!("run_in_child.rs");

pub(crate) unsafe fn replacement_survives(fd: libc::c_int, release: impl FnOnce()) {
    unsafe {
        assert_eq!(libc::fcntl(fd, libc::F_GETFD), -1);
        let replacement = std::thread::spawn(move || {
            let replacement = libc::open(c"/dev/null".as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC);
            assert_eq!(replacement, fd);
            replacement
        })
        .join()
        .unwrap();
        assert_eq!(libc::fcntl(replacement, libc::F_GETFD), libc::FD_CLOEXEC);
        let mut info: libc::stat = std::mem::zeroed();
        assert_eq!(libc::fstat(replacement, &raw mut info), 0);
        assert_eq!(info.st_mode & libc::S_IFMT, libc::S_IFCHR);
        release();
        assert_eq!(libc::fcntl(replacement, libc::F_GETFD), libc::FD_CLOEXEC);
        assert_eq!(libc::close(replacement), 0);
    }
}
