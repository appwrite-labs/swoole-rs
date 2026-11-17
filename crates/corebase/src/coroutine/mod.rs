//! The coroutine scheduler and per-coroutine state, Channel, each coroutine's
//! context (its stack and the switch onto it) and `swoole::Coroutine`'s
//! operations. C++ runs the effects: swap callbacks, timers and destructors.
pub mod chan;
pub mod context;
mod fcontext;
mod state;
pub use state::*;
#[cfg(feature = "upstream-symbols")]
mod facade;
#[cfg(feature = "upstream-symbols")]
pub use facade::{
    Host, activate, cancel, deactivate, print_socket_bound_error, set_host, spawn,
    swr_v1_coroutine_create, yield_ex, yield_resumed, yield_timed, yield_timed_armed,
};
#[cfg(feature = "upstream-symbols")]
pub use facade::{print_list, swoole_coroutine_iterator_each, swoole_coroutine_iterator_reset};
mod live;
pub use live::*;
mod scheduler;
pub use scheduler::{
    Counters, clear_current_task, coroutine_cid_of, coroutine_count, coroutine_get,
    coroutine_older, coroutine_state, counters, current, current_cid, current_native,
    current_resume_code, current_safe, park, resume, scheduler, set_callback, set_current_task,
    set_socket_bound_cid, set_stack_size, socket_bound_cid, suspend, swr_v1_coroutine_bail,
    yield_with_cancel, yield_with_cancel_armed,
};
mod stack;
pub use stack::*;
mod thread;
pub use thread::{
    THREAD, swr_v1_thread_context_clean, swr_v1_thread_context_init, thread_context_active,
};
