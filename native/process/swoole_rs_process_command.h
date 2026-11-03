#ifndef SWOOLE_RS_PROCESS_COMMAND_H
#define SWOOLE_RS_PROCESS_COMMAND_H
/* Server::command's routing (crates/process/src/command.rs): native code fills
 * what it knows, Rust answers where the command goes or which notice rejects it,
 * and native code keeps the command tables and performs the route. */
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif

enum {
    SWR_COMMAND_FAIL = 0,
    SWR_COMMAND_TO_REACTOR_THREAD = 1,
    SWR_COMMAND_TO_EVENT_WORKER = 2,
    SWR_COMMAND_TO_TASK_WORKER = 3,
    SWR_COMMAND_TO_MANAGER = 4,
    SWR_COMMAND_TO_MASTER = 5,
};

enum {
    SWR_COMMAND_LOG_NONE = 0,
    SWR_COMMAND_LOG_UNKNOWN = 1,
    SWR_COMMAND_LOG_NOT_MASTER = 2,
    SWR_COMMAND_LOG_NOT_WORKER_0 = 3,
    SWR_COMMAND_LOG_UNSUPPORTED_TYPE = 4,
    SWR_COMMAND_LOG_UNSUPPORTED_MODE = 5,
    SWR_COMMAND_LOG_INVALID_THREAD = 6,
    SWR_COMMAND_LOG_INVALID_WORKER = 7,
    SWR_COMMAND_LOG_INVALID_TASK_WORKER = 8,
    SWR_COMMAND_LOG_TOO_LARGE = 9,
};

typedef struct {
    bool started;
    bool found; /* commands.find(name) */
    bool process_mode;
    bool base_mode;
    bool master;
    int accepted_types;
    uint32_t worker_id; /* base mode: this worker's id */
    uint32_t process_id;
    int process_type;
    uint32_t reactor_num;
    uint32_t worker_num;
    uint32_t task_worker_num;
    size_t length;           /* the message's */
    size_t manager_capacity; /* sizeof(EventData::data) */
} swr_v1_process_command_input;

typedef struct {
    int action;          /* SWR_COMMAND_TO_*, or SWR_COMMAND_FAIL */
    int log;             /* SWR_COMMAND_LOG_* to report first */
    uint32_t process_id; /* once aliases apply */
    bool request;        /* a request id is taken and the callback stored (erased again on failure) */
} swr_v1_process_command_route;

void swr_v1_process_route_command(const swr_v1_process_command_input *input, swr_v1_process_command_route *output);
#ifdef __cplusplus
}
#endif
#endif
