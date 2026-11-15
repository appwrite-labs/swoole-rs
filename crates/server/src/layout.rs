//! Checks every borrowed native wire view before it is used.
use core::mem::{align_of, offset_of, size_of};

const RECORD_0_OFFSETS: &[usize] = &[
    offset_of!(crate::config::ServerConfig, reactor_num),
    offset_of!(crate::config::ServerConfig, worker_num),
    offset_of!(crate::config::ServerConfig, dgram_port_num),
    offset_of!(crate::config::ServerConfig, dispatch_mode),
    offset_of!(crate::config::ServerConfig, scheduler_warning),
    offset_of!(crate::config::ServerConfig, max_request),
    offset_of!(crate::config::ServerConfig, max_request_grace),
    offset_of!(crate::config::ServerConfig, udp_socket_ipv4),
    offset_of!(crate::config::ServerConfig, udp_socket_ipv6),
    offset_of!(crate::config::ServerConfig, dgram_socket),
    offset_of!(crate::config::ServerConfig, max_wait_time),
    offset_of!(crate::config::ServerConfig, worker_max_concurrency),
    offset_of!(crate::config::ServerConfig, daemonize),
    offset_of!(crate::config::ServerConfig, have_dgram_sock),
    offset_of!(crate::config::ServerConfig, have_stream_sock),
    offset_of!(crate::config::ServerConfig, open_cpu_affinity),
    offset_of!(crate::config::ServerConfig, disable_notify),
    offset_of!(crate::config::ServerConfig, discard_timeout_request),
    offset_of!(crate::config::ServerConfig, http_parse_cookie),
    offset_of!(crate::config::ServerConfig, http_parse_post),
    offset_of!(crate::config::ServerConfig, http_parse_files),
    offset_of!(crate::config::ServerConfig, http_compression),
    offset_of!(crate::config::ServerConfig, websocket_compression),
    offset_of!(crate::config::ServerConfig, enable_static_handler),
    offset_of!(crate::config::ServerConfig, http_autoindex),
    offset_of!(crate::config::ServerConfig, enable_unsafe_event),
    offset_of!(crate::config::ServerConfig, enable_delay_receive),
    offset_of!(crate::config::ServerConfig, enable_reuse_port),
    offset_of!(crate::config::ServerConfig, reload_async),
    offset_of!(crate::config::ServerConfig, event_object),
    offset_of!(crate::config::ServerConfig, task_object),
    offset_of!(crate::config::ServerConfig, task_enable_coroutine),
    offset_of!(crate::config::ServerConfig, send_yield),
    offset_of!(crate::config::ServerConfig, enable_coroutine),
    offset_of!(crate::config::ServerConfig, single_thread),
    offset_of!(crate::config::ServerConfig, running),
    offset_of!(crate::config::ServerConfig, cpu_affinity_available),
    offset_of!(crate::config::ServerConfig, pipe_command),
    offset_of!(crate::config::ServerConfig, send_timeout),
    offset_of!(crate::config::ServerConfig, heartbeat_check_interval),
    offset_of!(crate::config::ServerConfig, warning_time),
    offset_of!(crate::config::ServerConfig, timezone_),
    offset_of!(crate::config::ServerConfig, master_timer),
    offset_of!(crate::config::ServerConfig, heartbeat_timer),
    offset_of!(crate::config::ServerConfig, output_buffer_size),
    offset_of!(crate::config::ServerConfig, input_buffer_size),
    offset_of!(crate::config::ServerConfig, max_queued_bytes),
    offset_of!(crate::config::ServerConfig, ipc_max_size),
    offset_of!(crate::config::ServerConfig, private_data_1),
    offset_of!(crate::config::ServerConfig, private_data_2),
    offset_of!(crate::config::ServerConfig, private_data_3),
    offset_of!(crate::config::ServerConfig, private_data_4),
    offset_of!(crate::config::ServerConfig, factory_),
    offset_of!(crate::config::ServerConfig, manager_),
    offset_of!(crate::config::ServerConfig, task_worker_num),
    offset_of!(crate::config::ServerConfig, task_ipc_mode),
    offset_of!(crate::config::ServerConfig, task_max_request),
    offset_of!(crate::config::ServerConfig, task_max_request_grace),
    offset_of!(crate::config::ServerConfig, task_results),
    offset_of!(crate::config::ServerConfig, worker_registry),
    offset_of!(crate::config::ServerConfig, user_workers),
    offset_of!(crate::config::ServerConfig, workers),
    offset_of!(crate::config::ServerConfig, gs),
    offset_of!(crate::config::ServerConfig, upload_max_filesize),
    offset_of!(crate::config::ServerConfig, http_compression_level),
    offset_of!(crate::config::ServerConfig, compression_min_length),
    offset_of!(crate::config::ServerConfig, recv_buffer_allocator),
    offset_of!(crate::config::ServerConfig, recv_buffer_size),
    offset_of!(crate::config::ServerConfig, message_queue_key),
    offset_of!(crate::config::ServerConfig, mode_),
];

const RECORD_1_OFFSETS: &[usize] = &[
    offset_of!(crate::records::ServerTables, connection_list),
    offset_of!(crate::records::ServerTables, session_list),
    offset_of!(crate::records::ServerTables, port_gs_list),
    offset_of!(crate::records::ServerTables, max_connection),
    offset_of!(crate::records::ServerTables, enable_accept_timer),
    offset_of!(crate::records::ServerTables, reactor_pipe_num),
    crate::records::REACTOR_THREADS_OFFSET,
    offset_of!(crate::records::ServerTables, ports),
    offset_of!(crate::records::ServerTables, message_bus),
];

const RECORD_2_OFFSETS: &[usize] = &[
    offset_of!(crate::records::SessionRecord, id),
    offset_of!(crate::records::SessionRecord, fd),
];

const RECORD_3_OFFSETS: &[usize] = &[
    offset_of!(crate::records::ConnectionRecord, active),
    offset_of!(crate::records::ConnectionRecord, socket_type),
    offset_of!(crate::records::ConnectionRecord, fd),
    offset_of!(crate::records::ConnectionRecord, worker_id),
    offset_of!(crate::records::ConnectionRecord, session_id),
    offset_of!(crate::records::ConnectionRecord, ssl),
    offset_of!(crate::records::ConnectionRecord, ssl_ready),
    offset_of!(crate::records::ConnectionRecord, overflow),
    offset_of!(crate::records::ConnectionRecord, high_watermark),
    offset_of!(crate::records::ConnectionRecord, http_upgrade),
    offset_of!(crate::records::ConnectionRecord, http2_stream),
    offset_of!(crate::records::ConnectionRecord, websocket_compression),
    offset_of!(crate::records::ConnectionRecord, close_actively),
    offset_of!(crate::records::ConnectionRecord, close_queued),
    offset_of!(crate::records::ConnectionRecord, closing),
    offset_of!(crate::records::ConnectionRecord, close_reset),
    offset_of!(crate::records::ConnectionRecord, peer_closed),
    offset_of!(crate::records::ConnectionRecord, protect),
    offset_of!(crate::records::ConnectionRecord, close_notify),
    offset_of!(crate::records::ConnectionRecord, close_force),
    offset_of!(crate::records::ConnectionRecord, reactor_id),
    offset_of!(crate::records::ConnectionRecord, close_errno),
    offset_of!(crate::records::ConnectionRecord, recv_queued_bytes),
    offset_of!(crate::records::ConnectionRecord, send_queued_bytes),
    offset_of!(crate::records::ConnectionRecord, waiting_time),
    offset_of!(crate::records::ConnectionRecord, local_port),
    offset_of!(crate::records::ConnectionRecord, local_addr_index),
    offset_of!(crate::records::ConnectionRecord, timer),
    offset_of!(crate::records::ConnectionRecord, info),
    offset_of!(crate::records::ConnectionRecord, object),
    offset_of!(crate::records::ConnectionRecord, socket),
    offset_of!(crate::records::ConnectionRecord, connect_time),
    offset_of!(crate::records::ConnectionRecord, last_recv_time),
    offset_of!(crate::records::ConnectionRecord, last_send_time),
    offset_of!(crate::records::ConnectionRecord, last_dispatch_time),
    offset_of!(crate::records::ConnectionRecord, uid),
    offset_of!(crate::records::ConnectionRecord, websocket_status),
    offset_of!(crate::records::ConnectionRecord, websocket_buffer),
    offset_of!(crate::records::ConnectionRecord, ssl_client_cert),
    offset_of!(crate::records::ConnectionRecord, ssl_client_cert_pid),
    offset_of!(crate::records::ConnectionRecord, lock),
    offset_of!(crate::records::ConnectionRecord, closed),
    offset_of!(crate::records::ConnectionRecord, server_fd),
    crate::records::PORT_OFFSET,
];

const RECORD_4_OFFSETS: &[usize] = &[
    offset_of!(crate::port_config::PortConfig, object_id),
    offset_of!(crate::port_config::PortConfig, backlog),
    offset_of!(crate::port_config::PortConfig, listening),
    offset_of!(crate::port_config::PortConfig, tcp_defer_accept),
    offset_of!(crate::port_config::PortConfig, tcp_fastopen),
    offset_of!(crate::port_config::PortConfig, tcp_keepidle),
    offset_of!(crate::port_config::PortConfig, tcp_keepinterval),
    offset_of!(crate::port_config::PortConfig, tcp_keepcount),
    offset_of!(crate::port_config::PortConfig, tcp_user_timeout),
    offset_of!(crate::port_config::PortConfig, max_idle_time),
    offset_of!(crate::port_config::PortConfig, socket_buffer_size),
    offset_of!(crate::port_config::PortConfig, buffer_high_watermark),
    offset_of!(crate::port_config::PortConfig, buffer_low_watermark),
    offset_of!(crate::port_config::PortConfig, r#type),
    offset_of!(crate::port_config::PortConfig, ssl),
    offset_of!(crate::port_config::PortConfig, port),
    offset_of!(crate::port_config::PortConfig, socket),
    offset_of!(crate::port_config::PortConfig, thread_id),
    offset_of!(crate::port_config::PortConfig, heartbeat_idle_time),
    offset_of!(crate::port_config::PortConfig, open_eof_check),
    offset_of!(crate::port_config::PortConfig, open_http_protocol),
    offset_of!(crate::port_config::PortConfig, open_http2_protocol),
    offset_of!(crate::port_config::PortConfig, open_websocket_protocol),
    offset_of!(crate::port_config::PortConfig, open_length_check),
    offset_of!(crate::port_config::PortConfig, open_mqtt_protocol),
    offset_of!(crate::port_config::PortConfig, open_redis_protocol),
    offset_of!(crate::port_config::PortConfig, open_tcp_nodelay),
    offset_of!(crate::port_config::PortConfig, open_tcp_nopush),
    offset_of!(crate::port_config::PortConfig, open_tcp_keepalive),
    offset_of!(
        crate::port_config::PortConfig,
        kernel_socket_recv_buffer_size
    ),
    offset_of!(
        crate::port_config::PortConfig,
        kernel_socket_send_buffer_size
    ),
    offset_of!(crate::port_config::PortConfig, gs),
    offset_of!(crate::port_config::PortConfig, ptr),
    offset_of!(crate::port_config::PortConfig, websocket_frames),
    offset_of!(crate::port_config::PortConfig, websocket_protocol),
    offset_of!(crate::port_config::PortConfig, host),
    offset_of!(crate::port_config::PortConfig, protocol),
    offset_of!(crate::port_config::PortConfig, on_read),
    offset_of!(crate::port_config::PortConfig, ssl_context),
    offset_of!(crate::port_config::PortConfig, tls),
    offset_of!(crate::port_config::PortConfig, length_owner),
];

const RECORD_5_OFFSETS: &[usize] = &[
    offset_of!(crate::shared::ServerCounters, master_pid),
    offset_of!(crate::shared::ServerCounters, manager_pid),
    offset_of!(crate::shared::ServerCounters, session_round),
    offset_of!(crate::shared::ServerCounters, start),
    offset_of!(crate::shared::ServerCounters, shutdown),
    offset_of!(crate::shared::ServerCounters, max_fd),
    offset_of!(crate::shared::ServerCounters, min_fd),
    offset_of!(crate::shared::ServerCounters, onstart_called),
    offset_of!(crate::shared::ServerCounters, start_time),
    offset_of!(crate::shared::ServerCounters, connection_num),
    offset_of!(crate::shared::ServerCounters, connection_nums),
    offset_of!(crate::shared::ServerCounters, tasking_num),
    offset_of!(crate::shared::ServerCounters, max_concurrency),
    offset_of!(crate::shared::ServerCounters, concurrency),
    offset_of!(crate::shared::ServerCounters, abort_count),
    offset_of!(crate::shared::ServerCounters, accept_count),
    offset_of!(crate::shared::ServerCounters, close_count),
    offset_of!(crate::shared::ServerCounters, dispatch_count),
    offset_of!(crate::shared::ServerCounters, request_count),
    offset_of!(crate::shared::ServerCounters, response_count),
    offset_of!(crate::shared::ServerCounters, total_recv_bytes),
    offset_of!(crate::shared::ServerCounters, total_send_bytes),
    offset_of!(crate::shared::ServerCounters, pipe_packet_msg_id),
    offset_of!(crate::shared::ServerCounters, task_count),
    offset_of!(crate::shared::ServerCounters, spinlock),
];
const RECORD_6_OFFSETS: &[usize] = &[
    offset_of!(crate::shared::PortCounters, connection_num),
    offset_of!(crate::shared::PortCounters, connection_nums),
    offset_of!(crate::shared::PortCounters, abort_count),
    offset_of!(crate::shared::PortCounters, accept_count),
    offset_of!(crate::shared::PortCounters, close_count),
    offset_of!(crate::shared::PortCounters, dispatch_count),
    offset_of!(crate::shared::PortCounters, request_count),
    offset_of!(crate::shared::PortCounters, response_count),
    offset_of!(crate::shared::PortCounters, total_recv_bytes),
    offset_of!(crate::shared::PortCounters, total_send_bytes),
];

const RECORD_7_OFFSETS: &[usize] = &[0, size_of::<usize>(), 2 * size_of::<usize>()];

const RECORD_8_OFFSETS: &[usize] = &[
    offset_of!(crate::shared::ServerShared, pools),
    offset_of!(crate::shared::ServerShared, pools)
        + size_of::<swoole_rs_process::process_pool::PoolRecord>(),
    offset_of!(crate::shared::ServerShared, manager_barrier),
];

/// Checks a mirror of record `kind` (its size, alignment and field offsets) against
/// the canonical Rust record.
#[must_use]
pub fn server_record_layout(kind: u32, size: usize, alignment: usize, actual: &[usize]) -> bool {
    match kind {
        0 => {
            type Record = crate::config::ServerConfig;
            size == size_of::<Record>()
                && alignment == align_of::<Record>()
                && actual == RECORD_0_OFFSETS
        }
        1 => {
            type Record = crate::records::ServerTables;
            size == size_of::<Record>()
                && alignment == align_of::<Record>()
                && actual == RECORD_1_OFFSETS
        }
        2 => {
            type Record = crate::records::SessionRecord;
            size == size_of::<Record>()
                && alignment == align_of::<Record>()
                && actual == RECORD_2_OFFSETS
        }
        3 => {
            type Record = crate::records::ConnectionRecord;
            size == size_of::<Record>()
                && alignment == align_of::<Record>()
                && actual == RECORD_3_OFFSETS
        }
        4 => {
            type Record = crate::port_config::PortConfig;
            size == size_of::<Record>()
                && alignment == align_of::<Record>()
                && actual == RECORD_4_OFFSETS
        }
        5 => {
            type Record = crate::shared::ServerCounters;
            size == size_of::<Record>()
                && alignment == align_of::<Record>()
                && actual == RECORD_5_OFFSETS
        }
        6 => {
            type Record = crate::shared::PortCounters;
            size == size_of::<Record>()
                && alignment == align_of::<Record>()
                && actual == RECORD_6_OFFSETS
        }
        7 => {
            type Record = crate::callbacks::ServerCallbacks;
            size == size_of::<Record>()
                && alignment == align_of::<Record>()
                && actual == RECORD_7_OFFSETS
        }
        8 => {
            type Record = crate::shared::ServerShared;
            size == size_of::<Record>()
                && alignment == align_of::<Record>()
                && actual == RECORD_8_OFFSETS
        }
        _ => false,
    }
}
