#ifndef SWOOLE_RS_SERVER_VIEW_H
#define SWOOLE_RS_SERVER_VIEW_H
#include "swoole_rs_server_state.h"
#include "swoole_rs_corebase_string_slot.h"
#include <cstdlib>
#include <type_traits>
#include <utility>

namespace swoole { namespace detail {
// A view of one Rust-owned callback slot. Assigning a callable boxes it, and the slot's
// `drop` frees the box: building and destroying a C++ lambda is the one part Rust can't
// do. Rust may store its own `invoke` with a null `ctx`.
template <class Sig> class Callback;
template <class R, class... A> class Callback<R(A...)> : swr_v1_server_callback {
    template <class T> static R call(void *boxed, A... args) {
        return static_cast<R>((*static_cast<T *>(boxed))(std::forward<A>(args)...));
    }
    template <class T> static bool engaged(const T &f, std::true_type) {
        return static_cast<bool>(f);
    }
    template <class T> static bool engaged(const T &, std::false_type) {
        return true;
    }
    template <class T> static void destroy(void *boxed) noexcept {
        delete static_cast<T *>(boxed);
    }
    void clear() {
        auto *old_drop = drop;
        auto *old_ctx = ctx;
        invoke = nullptr;
        ctx = nullptr;
        drop = nullptr;
        if (old_drop) old_drop(old_ctx);
    }

  public:
    Callback() = delete;
    Callback(const Callback &) = delete;
    Callback &operator=(const Callback &) = delete;
    Callback &operator=(std::nullptr_t) {
        clear();
        return *this;
    }
    // A null function pointer or an empty std::function empties the slot, as it
    // empties a std::function.
    template <class F> Callback &operator=(F &&f) {
        using T = typename std::decay<F>::type;
        T *boxed = engaged<T>(f, std::is_constructible<bool, const T &>{}) ? new T(std::forward<F>(f)) : nullptr;
        clear();
        if (boxed) {
            ctx = boxed;
            drop = &destroy<T>;
            invoke = reinterpret_cast<void (*)(void)>(&call<T>);
        }
        return *this;
    }
    explicit operator bool() const {
        return invoke != nullptr;
    }
    R operator()(A... args) const {
        return reinterpret_cast<R (*)(void *, A...)>(invoke)(ctx, std::forward<A>(args)...);
    }
    friend bool operator==(const Callback &c, std::nullptr_t) {
        return !c;
    }
    friend bool operator!=(const Callback &c, std::nullptr_t) {
        return bool(c);
    }
};

// Borrowed original API views over the configuration allocation owned by Rust.
// The base owner outlives every derived native callback/container during teardown.
class ServerFields {
  protected:
    swr_v1_server_state *state_;
    swr_v1_server_config *config_;
    swr_v1_server_tables *tables_;
    swr_v1_server_callback *callbacks_;
    // `Callback<Sig> &onX = callback(SWR_SERVER_CB_X);` binds a member to its slot.
    struct Slot {
        swr_v1_server_callback *slot;
        template <class Sig> operator Callback<Sig> &() const {
            return *reinterpret_cast<Callback<Sig> *>(slot);
        }
    };
    Slot callback(int index) const {
        return {callbacks_ + index};
    }
    const swr_v1_core_string *text(size_t index) const {
        return swr_v1_server_text_slot(state_, index);
    }
  public:
#define SWR_SERVER_CONFIG_REFERENCE(type, name) decltype(swr_v1_server_config::name) &name;
    SWR_SERVER_CONFIG_FIELDS(SWR_SERVER_CONFIG_REFERENCE)
#undef SWR_SERVER_CONFIG_REFERENCE
    ServerFields(uint32_t cpu_num, uint32_t max_sockets, long timezone, const void *allocator)
        : state_(swr_v1_server_state_new()), config_(swr_v1_server_config_get(state_)),
          tables_(swr_v1_server_tables_get(state_)), callbacks_(swr_v1_server_callbacks_get(state_))
#define SWR_SERVER_CONFIG_BIND(type, name) , name(config_->name)
          SWR_SERVER_CONFIG_FIELDS(SWR_SERVER_CONFIG_BIND)
#undef SWR_SERVER_CONFIG_BIND
    {
        { const size_t offsets[] = {offsetof(swr_v1_server_config,reactor_num),offsetof(swr_v1_server_config,worker_num),offsetof(swr_v1_server_config,dgram_port_num),offsetof(swr_v1_server_config,dispatch_mode),offsetof(swr_v1_server_config,scheduler_warning),offsetof(swr_v1_server_config,max_request),offsetof(swr_v1_server_config,max_request_grace),offsetof(swr_v1_server_config,udp_socket_ipv4),offsetof(swr_v1_server_config,udp_socket_ipv6),offsetof(swr_v1_server_config,dgram_socket),offsetof(swr_v1_server_config,max_wait_time),offsetof(swr_v1_server_config,worker_max_concurrency),offsetof(swr_v1_server_config,daemonize),offsetof(swr_v1_server_config,have_dgram_sock),offsetof(swr_v1_server_config,have_stream_sock),offsetof(swr_v1_server_config,open_cpu_affinity),offsetof(swr_v1_server_config,disable_notify),offsetof(swr_v1_server_config,discard_timeout_request),offsetof(swr_v1_server_config,http_parse_cookie),offsetof(swr_v1_server_config,http_parse_post),offsetof(swr_v1_server_config,http_parse_files),offsetof(swr_v1_server_config,http_compression),offsetof(swr_v1_server_config,websocket_compression),offsetof(swr_v1_server_config,enable_static_handler),offsetof(swr_v1_server_config,http_autoindex),offsetof(swr_v1_server_config,enable_unsafe_event),offsetof(swr_v1_server_config,enable_delay_receive),offsetof(swr_v1_server_config,enable_reuse_port),offsetof(swr_v1_server_config,reload_async),offsetof(swr_v1_server_config,event_object),offsetof(swr_v1_server_config,task_object),offsetof(swr_v1_server_config,task_enable_coroutine),offsetof(swr_v1_server_config,send_yield),offsetof(swr_v1_server_config,enable_coroutine),offsetof(swr_v1_server_config,single_thread),offsetof(swr_v1_server_config,running),offsetof(swr_v1_server_config,cpu_affinity_available),offsetof(swr_v1_server_config,cpu_affinity_available_num),offsetof(swr_v1_server_config,pipe_command),offsetof(swr_v1_server_config,send_timeout),offsetof(swr_v1_server_config,heartbeat_check_interval),offsetof(swr_v1_server_config,warning_time),offsetof(swr_v1_server_config,timezone_),offsetof(swr_v1_server_config,master_timer),offsetof(swr_v1_server_config,heartbeat_timer),offsetof(swr_v1_server_config,output_buffer_size),offsetof(swr_v1_server_config,input_buffer_size),offsetof(swr_v1_server_config,max_queued_bytes),offsetof(swr_v1_server_config,ipc_max_size),offsetof(swr_v1_server_config,private_data_1),offsetof(swr_v1_server_config,private_data_2),offsetof(swr_v1_server_config,private_data_3),offsetof(swr_v1_server_config,private_data_4),offsetof(swr_v1_server_config,factory_),offsetof(swr_v1_server_config,manager_),offsetof(swr_v1_server_config,task_worker_num),offsetof(swr_v1_server_config,task_ipc_mode),offsetof(swr_v1_server_config,task_max_request),offsetof(swr_v1_server_config,task_max_request_grace),offsetof(swr_v1_server_config,task_results),offsetof(swr_v1_server_config,worker_registry),offsetof(swr_v1_server_config,user_workers),offsetof(swr_v1_server_config,workers),offsetof(swr_v1_server_config,gs),offsetof(swr_v1_server_config,upload_max_filesize),offsetof(swr_v1_server_config,http_compression_level),offsetof(swr_v1_server_config,compression_min_length),offsetof(swr_v1_server_config,recv_buffer_allocator),offsetof(swr_v1_server_config,recv_buffer_size),offsetof(swr_v1_server_config,manager_alarm),offsetof(swr_v1_server_config,message_queue_key),offsetof(swr_v1_server_config,mode_)};
          if (!swr_v1_server_record_layout(0, sizeof(swr_v1_server_config), alignof(swr_v1_server_config), offsets, sizeof(offsets)/sizeof(size_t))) std::abort(); }
        { const size_t offsets[] = {offsetof(swr_v1_server_tables,connection_list),offsetof(swr_v1_server_tables,session_list),offsetof(swr_v1_server_tables,port_gs_list),offsetof(swr_v1_server_tables,max_connection),offsetof(swr_v1_server_tables,enable_accept_timer),offsetof(swr_v1_server_tables,reactor_pipe_num),offsetof(swr_v1_server_tables,reactor_threads),offsetof(swr_v1_server_tables,ports)};
          if (!swr_v1_server_record_layout(1, sizeof(swr_v1_server_tables), alignof(swr_v1_server_tables), offsets, sizeof(offsets)/sizeof(size_t))) std::abort(); }
        { const size_t offsets[] = {offsetof(Session,id),offsetof(Session,fd)};
          if (!swr_v1_server_record_layout(2, sizeof(Session), alignof(Session), offsets, sizeof(offsets)/sizeof(size_t))) std::abort(); }
        { const size_t offsets[] = {offsetof(Connection,active),offsetof(Connection,socket_type),offsetof(Connection,fd),offsetof(Connection,worker_id),offsetof(Connection,session_id),offsetof(Connection,ssl),offsetof(Connection,ssl_ready),offsetof(Connection,overflow),offsetof(Connection,high_watermark),offsetof(Connection,http_upgrade),offsetof(Connection,http2_stream),offsetof(Connection,websocket_compression),offsetof(Connection,close_actively),offsetof(Connection,close_queued),offsetof(Connection,closing),offsetof(Connection,close_reset),offsetof(Connection,peer_closed),offsetof(Connection,protect),offsetof(Connection,close_notify),offsetof(Connection,close_force),offsetof(Connection,reactor_id),offsetof(Connection,close_errno),offsetof(Connection,recv_queued_bytes),offsetof(Connection,send_queued_bytes),offsetof(Connection,waiting_time),offsetof(Connection,local_port),offsetof(Connection,local_addr_index),offsetof(Connection,timer),offsetof(Connection,info),offsetof(Connection,object),offsetof(Connection,socket),offsetof(Connection,connect_time),offsetof(Connection,last_recv_time),offsetof(Connection,last_send_time),offsetof(Connection,last_dispatch_time),offsetof(Connection,uid),offsetof(Connection,websocket_status),offsetof(Connection,websocket_buffer),offsetof(Connection,ssl_client_cert),offsetof(Connection,ssl_client_cert_pid),offsetof(Connection,lock),offsetof(Connection,closed),offsetof(Connection,server_fd)};
          if (!swr_v1_server_record_layout(3, sizeof(Connection), alignof(Connection), offsets, sizeof(offsets)/sizeof(size_t))) std::abort(); }
        { const size_t offsets[] = {offsetof(ServerCounters,master_pid),offsetof(ServerCounters,manager_pid),offsetof(ServerCounters,session_round),offsetof(ServerCounters,start),offsetof(ServerCounters,shutdown),offsetof(ServerCounters,max_fd),offsetof(ServerCounters,min_fd),offsetof(ServerCounters,onstart_called),offsetof(ServerCounters,start_time),offsetof(ServerCounters,connection_num),offsetof(ServerCounters,connection_nums),offsetof(ServerCounters,tasking_num),offsetof(ServerCounters,max_concurrency),offsetof(ServerCounters,concurrency),offsetof(ServerCounters,abort_count),offsetof(ServerCounters,accept_count),offsetof(ServerCounters,close_count),offsetof(ServerCounters,dispatch_count),offsetof(ServerCounters,request_count),offsetof(ServerCounters,response_count),offsetof(ServerCounters,total_recv_bytes),offsetof(ServerCounters,total_send_bytes),offsetof(ServerCounters,pipe_packet_msg_id),offsetof(ServerCounters,task_count),offsetof(ServerCounters,spinlock)};
          if (!swr_v1_server_record_layout(5,sizeof(ServerCounters),alignof(ServerCounters),offsets,sizeof(offsets)/sizeof(size_t))) std::abort(); }
        { const size_t offsets[] = {offsetof(PortCounters,connection_num),offsetof(PortCounters,connection_nums),offsetof(PortCounters,abort_count),offsetof(PortCounters,accept_count),offsetof(PortCounters,close_count),offsetof(PortCounters,dispatch_count),offsetof(PortCounters,request_count),offsetof(PortCounters,response_count),offsetof(PortCounters,total_recv_bytes),offsetof(PortCounters,total_send_bytes)};
          if (!swr_v1_server_record_layout(6,sizeof(PortCounters),alignof(PortCounters),offsets,sizeof(offsets)/sizeof(size_t))) std::abort(); }
        { const size_t offsets[] = {offsetof(swr_v1_server_callback,invoke),offsetof(swr_v1_server_callback,ctx),offsetof(swr_v1_server_callback,drop)};
          if (!swr_v1_server_record_layout(7,sizeof(swr_v1_server_callback)*SWR_SERVER_CB_COUNT,alignof(swr_v1_server_callback),offsets,sizeof(offsets)/sizeof(size_t))) std::abort(); }
        swr_v1_server_config_init(state_, cpu_num, max_sockets, timezone, allocator);
    }
    swr_v1_server_state *state_handle() const { return state_; }
    ~ServerFields() { swr_v1_server_state_free(state_); }
    ServerFields(const ServerFields &) = delete;
    ServerFields &operator=(const ServerFields &) = delete;
};
} }
#endif
