#ifndef SWOOLE_RS_SERVER_RECORDS_H
#define SWOOLE_RS_SERVER_RECORDS_H
/* Wire views of crates/server/src/records.rs. Include after the native address/types. */
#ifdef __cplusplus
namespace swoole {
struct Session {
    SessionId id;
    int fd;
    uint32_t reactor_id : 8;
    uint32_t reserve_ : 24;
};
struct Connection {
    uint8_t active;
    SocketType socket_type;
    int fd;
    int worker_id;
    SessionId session_id;
    uint8_t ssl;
    uint8_t ssl_ready;
    uint8_t overflow;
    uint8_t high_watermark;
    uint8_t http_upgrade;
    uint8_t http2_stream;
    uint8_t websocket_compression;
    uint8_t close_actively;
    uint8_t close_queued;
    uint8_t closing;
    uint8_t close_reset;
    uint8_t peer_closed;
    uint8_t protect;
    uint8_t close_notify;
    uint8_t close_force;
    ReactorId reactor_id;
    uint16_t close_errno;
    sw_atomic_t recv_queued_bytes;
    uint32_t send_queued_bytes;
    uint16_t waiting_time;
    uint16_t local_port;
    uint16_t local_addr_index;
    TimerNode * timer;
    network::Address info;
    void * object;
    network::Socket * socket;
    double connect_time;
    double last_recv_time;
    double last_send_time;
    double last_dispatch_time;
    uint32_t uid;
    uint8_t websocket_status;
    String * websocket_buffer;
    String * ssl_client_cert;
    pid_t ssl_client_cert_pid;
    sw_atomic_t lock;
    sw_atomic_t closed;
    int server_fd;
};
}
#endif
#endif
