//! `TEST(ringbuffer, thread)` in its own test binary: cargo runs test binaries one at
//! a time, as upstream's runner runs its tests, so the crate's other threaded tests
//! can't starve the writer (under `ASan` they did, #1734). The writer retries a full
//! ring until a deadline rather than upstream's 100 tries of 10us: under load a
//! reader can fall more than a millisecond behind (#2037).
use std::{mem, ptr};
use swoole_rs_memory::{
    members::{
        swr_v1_memory_ring_buffer_alloc, swr_v1_memory_ring_buffer_delete,
        swr_v1_memory_ring_buffer_free, swr_v1_memory_ring_buffer_new,
    },
    ring::Storage,
};

/// `TEST(ringbuffer, thread)`: one writer allocates 10000 records of 10000..99999
/// bytes from a 4 MiB ring (shared, then private), stamps each with its size and a
/// serial number, and sends it over one of 4 datagram socket pairs; each reader checks
/// both stamps and frees the record.
#[test]
fn ringbuffer_thread() {
    use std::os::unix::net::UnixDatagram;
    const READ_THREAD_N: usize = 4;
    const WRITE_N: usize = 10000;
    const PACKET_LEN: u32 = 90000;
    /// `pkg`: `id`, `size`, `serial_num`, `ptr`.
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Pkg {
        id: u32,
        size: u32,
        serial_num: u32,
        ptr: usize,
    }
    const PKG: usize = mem::size_of::<Pkg>();
    for shared in [true, false] {
        let mut ring = ptr::null_mut();
        assert_eq!(
            unsafe { swr_v1_memory_ring_buffer_new(1024 * 1024 * 4, shared, &raw mut ring) },
            0
        );
        let address = ring.addr();
        let mut pipes = Vec::new();
        let mut readers = Vec::new();
        for _ in 0..READ_THREAD_N {
            let (write, read) = UnixDatagram::pair().unwrap();
            pipes.push(write);
            readers.push(std::thread::spawn(move || {
                let ring = ptr::with_exposed_provenance_mut::<Storage>(address);
                for _ in 0..WRITE_N / READ_THREAD_N {
                    let mut bytes = [0u8; PKG];
                    assert_eq!(read.recv(&mut bytes).unwrap(), PKG);
                    let pkg = unsafe { bytes.as_ptr().cast::<Pkg>().read_unaligned() };
                    let data = ptr::with_exposed_provenance_mut::<u8>(pkg.ptr);
                    assert_eq!(unsafe { data.cast::<u32>().read_unaligned() }, pkg.size);
                    let tail = unsafe { data.add(pkg.size as usize - 4) };
                    assert_eq!(
                        unsafe { tail.cast::<u32>().read_unaligned() },
                        pkg.serial_num
                    );
                    unsafe { swr_v1_memory_ring_buffer_free(ring, data.cast()) };
                }
            }));
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
        for i in 0..WRITE_N {
            let size = 10000 + unsafe { libc::rand() }.unsigned_abs() % PACKET_LEN;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            let mut data = unsafe { swr_v1_memory_ring_buffer_alloc(ring, size) };
            while data.is_null() && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_micros(10));
                data = unsafe { swr_v1_memory_ring_buffer_alloc(ring, size) };
            }
            assert!(!data.is_null(), "alloc failed. index={i}, break");
            let pkg = Pkg {
                id: u32::try_from(i).unwrap(),
                size,
                serial_num: unsafe { libc::rand() }.unsigned_abs(),
                ptr: data.expose_provenance(),
            };
            let data = data.cast::<u8>();
            unsafe { data.cast::<u32>().write_unaligned(size) };
            unsafe {
                data.add(size as usize - 4)
                    .cast::<u32>()
                    .write_unaligned(pkg.serial_num);
            }
            let bytes =
                unsafe { std::slice::from_raw_parts(ptr::from_ref(&pkg).cast::<u8>(), PKG) };
            assert!(pipes[i % READ_THREAD_N].send(bytes).is_ok());
        }
        for reader in readers {
            reader.join().unwrap();
        }
        unsafe { swr_v1_memory_ring_buffer_delete(ring) };
    }
}
