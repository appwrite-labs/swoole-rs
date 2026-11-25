//! A coroutine parked on the reactor: `System::wait_event`, `socket_poll` and the
//! hooked `poll`, the coroutine `waitpid`, and the readiness handlers that resume
//! them. The loop below knows none of them; each new reactor gets the handlers
//! through `reactor::readiness::set_init_reactor`.
pub mod readiness;
pub mod socket_poll;
pub mod wait_event;
pub mod wait_upstream;
