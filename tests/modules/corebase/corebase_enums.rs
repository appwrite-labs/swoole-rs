// SPDX-License-Identifier: Apache-2.0
//! The corebase identifiers no C header exports, as `NAME VALUE` lines that
//! `constants.py` compares with the pinned `swoole.h`.
use swoole_rs_corebase::constants::{
    DNSLookupFlag, EventDataFlag, EventInitFlag, EventType, PipeType, ResultCode, ReturnCode,
    SW_PIPE_READ, SW_PIPE_WRITE, SocketFlag, SslState, TaskFlag, TimeoutType, TraverseOperation,
};

include!("enum_table.rs");

#[expect(clippy::print_stdout, reason = "constants.py reads the table")]
fn main() {
    enum_table! {
        SslState { SW_SSL_STATE_HANDSHAKE, SW_SSL_STATE_READY, SW_SSL_STATE_WAIT_STREAM }
        ResultCode { SW_OK, SW_ERR }
        ReturnCode {
            SW_SUCCESS, SW_CONTINUE, SW_WAIT, SW_CLOSE, SW_ERROR, SW_READY, SW_INVALID,
            SW_REDUCE_SIZE,
        }
        SocketFlag { SW_SOCK_NONBLOCK, SW_SOCK_CLOEXEC, SW_SOCK_SSL }
        TimeoutType {
            SW_TIMEOUT_DNS, SW_TIMEOUT_CONNECT, SW_TIMEOUT_READ, SW_TIMEOUT_WRITE,
            SW_TIMEOUT_RDWR, SW_TIMEOUT_ALL,
        }
        EventType {
            SW_EVENT_NULL, SW_EVENT_DEAULT, SW_EVENT_READ, SW_EVENT_WRITE, SW_EVENT_RDWR,
            SW_EVENT_ERROR, SW_EVENT_ONCE,
        }
        TraverseOperation { SW_TRAVERSE_KEEP, SW_TRAVERSE_REMOVE, SW_TRAVERSE_STOP }
        EventDataFlag {
            SW_EVENT_DATA_NORMAL, SW_EVENT_DATA_PTR, SW_EVENT_DATA_CHUNK, SW_EVENT_DATA_BEGIN,
            SW_EVENT_DATA_END, SW_EVENT_DATA_OBJ_PTR, SW_EVENT_DATA_POP_PTR,
        }
        TaskFlag {
            SW_TASK_TMPFILE, SW_TASK_SERIALIZE, SW_TASK_NONBLOCK, SW_TASK_CALLBACK,
            SW_TASK_WAITALL, SW_TASK_COROUTINE, SW_TASK_PEEK, SW_TASK_NOREPLY,
        }
        DNSLookupFlag { SW_DNS_LOOKUP_RANDOM }
        PipeType { SW_PIPE_WORKER, SW_PIPE_MASTER, SW_PIPE_NONBLOCK }
        EventInitFlag { SW_EVENTLOOP_WAIT_EXIT }
    }
    println!("SW_PIPE_READ {}", SW_PIPE_READ as i64);
    println!("SW_PIPE_WRITE {}", SW_PIPE_WRITE as i64);
}
