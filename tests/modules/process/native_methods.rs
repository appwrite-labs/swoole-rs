use super::tests::{Scripted, take_ops};
use crate::methods;
use crate::process_pool::{IpcMode, Op, PoolRecord, PoolRef, Protocol};
use crate::worker_record::WorkerType;

#[test]
fn methods_use_the_supplied_host_without_a_server_link_or_registration() {
    take_ops();
    methods::tests::install();
    let mut record = PoolRecord::empty();
    let pool = PoolRef::from(&mut record);
    methods::set_protocol(&Scripted, pool, Protocol::Stream);
    methods::set_start_id(&Scripted, pool, -1);
    methods::set_type(&Scripted, pool, WorkerType::Task);
    methods::set_max_request(&Scripted, pool, 10, 3);
    assert_eq!(pool.record().protocol_type.get(), Protocol::Stream.code());
    assert_eq!(pool.start_id(), u16::MAX);
    assert_eq!(pool.record().kind.get(), WorkerType::Task.code());
    assert_eq!((pool.max_request(), pool.max_request_grace()), (10, 3));
    assert_eq!(
        take_ops(),
        [(
            Op::MainLoop,
            u64::from(Protocol::Stream.code().cast_unsigned())
        )]
    );

    // Allocation refusal comes from the supplied host, which allocates nothing.
    swoole_rs_corebase::sys::set_errno(libc::ENOMEM);
    assert!(!methods::create(&Scripted, pool, 2, 0, 0));
    assert_eq!(take_ops(), []);
    assert_eq!(
        methods::tests::logged("ProcessPool::create"),
        [(0, 0, libc::ENOMEM, "malloc[1] failed".to_owned())]
    );
    assert_eq!(pool.worker_num(), 0);
}

#[test]
fn start_check_keeps_the_missing_listener_warning_with_a_supplied_host() {
    take_ops();
    methods::tests::install();
    let mut record = PoolRecord::empty();
    record.ipc_mode.set(IpcMode::Socket);
    let pool = PoolRef::from(&mut record);
    assert!(!methods::start_check(&Scripted, pool));
    assert_eq!(take_ops(), []);
    assert_eq!(
        methods::tests::logged("ProcessPool::start_check"),
        [(3, 4, 509, "must first listen to an tcp port".to_owned())]
    );
    assert!(!pool.running());
}
