#ifndef SWOOLE_RS_PROCESS_VIEWS_H
#define SWOOLE_RS_PROCESS_VIEWS_H
// Generated from crates/process/src/{worker_record,process_pool}.rs and crates/tls/src/socket_pair.rs.
// Typed native loans only; Rust owns the record layouts and operations.
#define SWR_WORKER_RECORD_FIELDS \
    int pid; \
    uint32_t id; \
    ProcessPool * pool; \
    swr_v1_os_msg_queue * queue; \
    bool shared; \
    bool redirect_stdout; \
    bool redirect_stdin; \
    bool redirect_stderr; \
    uint8_t status; \
    uint8_t type; \
    uint8_t msgqueue_mode; \
    bool child_process; \
    uint32_t concurrency; \
    time_t start_time; \
    sw_atomic_long_t dispatch_count; \
    sw_atomic_long_t request_count; \
    sw_atomic_long_t response_count; \
    size_t coroutine_num; \
    void * lock; \
    UnixSocket * pipe_object; \
    network::Socket * pipe_master; \
    network::Socket * pipe_worker; \
    network::Socket * pipe_current; \
    void * ptr;
#define SWR_POOL_RECORD_FIELDS \
    bool running; \
    bool reload_init; \
    bool read_message; \
    bool started; \
    bool schedule_by_sysvmsg; \
    bool async; \
    uint8_t ipc_mode; \
    ProtocolType protocol_type_; \
    int master_pid; \
    uint32_t max_wait_time; \
    uint64_t reload_count; \
    time_t reload_last_time; \
    uint8_t type; \
    uint16_t start_id; \
    bool use_msgqueue; \
    bool use_socket; \
    uint32_t max_packet_size_; \
    key_t msgqueue_key; \
    uint32_t worker_num; \
    uint32_t max_request; \
    uint32_t max_request_grace; \
    bool scheduler_warning; \
    time_t warning_time; \
    void (*onStart)(ProcessPool *); \
    void (*onShutdown)(ProcessPool *); \
    void (*onBeforeReload)(ProcessPool *); \
    void (*onAfterReload)(ProcessPool *); \
    int (*onTask)(ProcessPool *, Worker *, EventData *); \
    void (*onWorkerStart)(ProcessPool *, Worker *); \
    void (*onMessage)(ProcessPool *, RecvData *); \
    void (*onWorkerExit)(ProcessPool *, Worker *); \
    void (*onWorkerStop)(ProcessPool *, Worker *); \
    void (*onWorkerError)(ProcessPool *, Worker *, const ExitStatus &); \
    void (*onWorkerMessage)(ProcessPool *, EventData *); \
    int (*onWorkerNotFound)(ProcessPool *, const ExitStatus &); \
    int (*main_loop)(ProcessPool *, Worker *); \
    sw_atomic_t round_id; \
    Worker * workers; \
    void * pipes; \
    union { swr_v1_process_worker_registry *registry_; swr_v1_process_worker_registry *map_; }; \
    swr_v1_os_msg_queue * queue; \
    swr_v1_process_pool_ipc * ipc_; \
    void * message_box = nullptr; \
    void * message_bus = nullptr; \
    void * reload_task = nullptr; \
    void * ptr; \
    struct { uint64_t generation; long id; void *cancel; } kill_timer = {};
#define SWR_SOCKET_PAIR_RECORD_FIELDS \
    bool blocking; \
    double timeout; \
    int socks[2]; \
    network::Socket * master_socket; \
    network::Socket * worker_socket;
#endif
