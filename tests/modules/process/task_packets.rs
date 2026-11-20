use super::*;
use crate::buffer::string::OwnedString;
use std::ffi::CString;
use std::path::{Path, PathBuf};

const LARGE: usize = size_of::<TaskBuffer>() * 2;

fn directory(case: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("swr-{case}-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    dir
}

fn template(dir: &Path) -> CString {
    CString::new(dir.join("task.XXXXXX").as_os_str().as_bytes()).unwrap()
}

fn pack(dir: &Path, data: &[u8]) -> Option<TaskBuffer> {
    let mut task = TaskBuffer::default();
    let packed = pack_payload(&mut task.data, data, || template(dir));
    if !packed.ok {
        return None;
    }
    task.head.len = packed.info_len.try_into().unwrap();
    if packed.tmpfile {
        task.add_flags(TaskFlag::SW_TASK_TMPFILE.bits());
    }
    Some(task)
}

fn file_packet(dir: &Path, data: &[u8]) -> TaskBuffer {
    let path = dir.join("payload");
    std::fs::write(&path, data).unwrap();
    let name = path.as_os_str().as_bytes();
    let mut packet = PacketTask {
        length: data.len(),
        tmpfile: [0; TASK_TMP_PATH_SIZE],
    };
    packet.tmpfile[..name.len()].copy_from_slice(name);
    let mut task = TaskBuffer::default();
    packet.write(&mut task.data);
    task.head.len = size_of::<PacketTask>().try_into().unwrap();
    task.add_flags(TaskFlag::SW_TASK_TMPFILE.bits());
    task
}

#[test]
fn inline_lengths_must_fit_storage() {
    let mut bytes = vec![b'v'; size_of::<DataHead>() + IPC_PAYLOAD + 1];
    let mut buffer = OwnedString::default();
    for size in [IPC_PAYLOAD + 1, 0, IPC_PAYLOAD] {
        bytes[..size_of::<DataHead>()].fill(0);
        let at = offset_of!(DataHead, len);
        bytes[at..at + 4].copy_from_slice(&u32::try_from(size).unwrap().to_ne_bytes());
        let record = Packed::from_bytes(&bytes).unwrap();
        let read = record.unpack(&mut buffer);
        if size > IPC_PAYLOAD {
            assert!(read.is_none());
        } else {
            let (data, _) = read.unwrap();
            assert_eq!(data, record.payload());
            assert_eq!(data.len(), size);
            assert_eq!(data.as_ptr(), record.payload().as_ptr());
        }
    }
}

#[test]
fn file_header_lengths_must_match() {
    let dir = directory("descriptor");
    let task = file_packet(&dir, &vec![b'x'; LARGE]);
    let mut bytes = task.packed().payload().to_vec();
    bytes.push(0);
    let size = size_of::<PacketTask>();
    for len in [0, size - 1, size + 1] {
        assert_eq!(
            PacketTask::read(&bytes[..len]),
            Err("invalid task packet length")
        );
        assert_eq!(dir.read_dir().unwrap().count(), 1);
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn file_paths_must_have_a_terminator() {
    let dir = directory("termination");
    let mut task = TaskBuffer::default();
    let prefix = dir.as_os_str().as_bytes();
    let mut name = [b'x'; TASK_TMP_PATH_SIZE];
    name[..prefix.len()].copy_from_slice(prefix);
    name[prefix.len()] = b'/';
    let mut pkg = PacketTask {
        length: 1,
        tmpfile: name,
    };
    let path = Path::new(std::ffi::OsStr::from_bytes(&name[..name.len() - 1]));
    std::fs::write(path, b"q").unwrap();
    task.head.len = size_of::<PacketTask>().try_into().unwrap();
    task.add_flags(TaskFlag::SW_TASK_TMPFILE.bits());
    pkg.write(&mut task.data);
    assert_eq!(
        PacketTask::read(task.packed().payload()),
        Err("unterminated task tmpfile path")
    );
    assert!(path.exists());
    pkg.tmpfile[TASK_TMP_PATH_SIZE - 1] = 0;
    pkg.write(&mut task.data);
    let mut buffer = OwnedString::default();
    assert_eq!(task.packed().unpack(&mut buffer).unwrap().0, b"q");
    std::fs::remove_dir(dir).unwrap();
}

#[test]
fn valid_file_packets_can_be_peeked_then_consumed() {
    let dir = directory("peek");
    let data = vec![b'x'; LARGE];
    let mut task = file_packet(&dir, &data);
    let mut buffer = OwnedString::default();
    task.add_flags(TaskFlag::SW_TASK_PEEK.bits());
    assert_eq!(task.packed().unpack(&mut buffer).unwrap().0, data);
    assert_eq!(dir.read_dir().unwrap().count(), 1);
    task.head.ext_flags &= !TaskFlag::SW_TASK_PEEK.bits();
    assert_eq!(task.packed().unpack(&mut buffer).unwrap().0, data);
    std::fs::remove_dir(dir).unwrap();
}

#[test]
#[cfg_attr(miri, ignore = "Miri has no mkostemp")]
fn successful_inline_and_file_packets_preserve_bytes() {
    let dir = directory("roundtrip");
    for size in [0, 3, LARGE] {
        let data: Vec<_> = (0u8..=255).cycle().take(size).collect();
        let task = pack(&dir, &data).unwrap();
        let mut buffer = OwnedString::default();
        assert_eq!(task.packed().unpack(&mut buffer).unwrap().0, data);
        assert_eq!(dir.read_dir().unwrap().count(), 0);
    }
    std::fs::remove_dir(dir).unwrap();
}

#[test]
#[cfg_attr(miri, ignore = "Miri has no mkostemp")]
fn creation_failure_preserves_the_error_and_leaves_no_files() {
    let dir = directory("missing");
    let mut task = TaskBuffer::default();
    let data = vec![b'x'; LARGE];
    let missing = template(&dir.join("missing"));
    assert!(!pack_payload(&mut task.data, &data, || &missing).ok);
    assert_eq!(errno(), libc::ENOENT);
    std::fs::remove_dir(dir).unwrap();
}

#[test]
#[cfg_attr(miri, ignore = "Miri has no subprocesses")]
fn failed_writes_leave_no_files() {
    if crate::utilities::isolated_process::run_in_child(
        "process::task_file::originals::failed_writes_leave_no_files",
    ) {
        return;
    }
    let dir = directory("failed-write");
    let data = vec![b'x'; LARGE];
    unsafe {
        let mut original: libc::rlimit = std::mem::zeroed();
        assert_eq!(libc::getrlimit(libc::RLIMIT_FSIZE, &raw mut original), 0);
        assert_ne!(libc::signal(libc::SIGXFSZ, libc::SIG_IGN), libc::SIG_ERR);
        for bytes in [0, 16] {
            assert!(original.rlim_cur >= bytes);
            let limit = libc::rlimit {
                rlim_cur: bytes,
                ..original
            };
            assert_eq!(libc::setrlimit(libc::RLIMIT_FSIZE, &raw const limit), 0);
            let mut task = TaskBuffer::default();
            let packed = pack_payload(&mut task.data, &data, || template(&dir));
            let error = errno();
            assert_eq!(libc::setrlimit(libc::RLIMIT_FSIZE, &raw const original), 0);
            assert!(!packed.ok);
            assert_eq!(error, libc::EFBIG);
            assert_eq!(dir.read_dir().unwrap().count(), 0);
        }
    }
    std::fs::remove_dir(dir).unwrap();
}
