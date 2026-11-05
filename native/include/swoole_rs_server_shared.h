#ifndef SWOOLE_RS_SERVER_SHARED_H
#define SWOOLE_RS_SERVER_SHARED_H
namespace swoole {
#define SWR_PORT_COUNTER_FIELDS(F) \
 F(sw_atomic_t,connection_num) \
 F(sw_atomic_t *,connection_nums) \
 F(sw_atomic_long_t,abort_count) \
 F(sw_atomic_long_t,accept_count) \
 F(sw_atomic_long_t,close_count) \
 F(sw_atomic_long_t,dispatch_count) \
 F(sw_atomic_long_t,request_count) \
 F(sw_atomic_long_t,response_count) \
 F(sw_atomic_long_t,total_recv_bytes) \
 F(sw_atomic_long_t,total_send_bytes)
#define SWR_COUNTER_DECLARE(type,name) type name;
struct PortCounters { SWR_PORT_COUNTER_FIELDS(SWR_COUNTER_DECLARE) };
#undef SWR_COUNTER_DECLARE
#define SWR_SERVER_COUNTER_FIELDS(F) \
 F(pid_t,master_pid) \
 F(pid_t,manager_pid) \
 F(SessionId,session_round) \
 F(sw_atomic_t,start) \
 F(sw_atomic_t,shutdown) \
 F(int,max_fd) \
 F(int,min_fd) \
 F(bool,onstart_called) \
 F(time_t,start_time) \
 F(sw_atomic_t,connection_num) \
 F(sw_atomic_t *,connection_nums) \
 F(sw_atomic_t,tasking_num) \
 F(uint32_t,max_concurrency) \
 F(sw_atomic_t,concurrency) \
 F(sw_atomic_long_t,abort_count) \
 F(sw_atomic_long_t,accept_count) \
 F(sw_atomic_long_t,close_count) \
 F(sw_atomic_long_t,dispatch_count) \
 F(sw_atomic_long_t,request_count) \
 F(sw_atomic_long_t,response_count) \
 F(sw_atomic_long_t,total_recv_bytes) \
 F(sw_atomic_long_t,total_send_bytes) \
 F(sw_atomic_long_t,pipe_packet_msg_id) \
 F(sw_atomic_long_t,task_count) \
 F(sw_atomic_t,spinlock)
#define SWR_COUNTER_DECLARE(type,name) type name;
struct ServerCounters { SWR_SERVER_COUNTER_FIELDS(SWR_COUNTER_DECLARE) };
#undef SWR_COUNTER_DECLARE
}
#endif
