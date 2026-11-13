//! `ServerEventType` values on the reactor pipe.
pub const EVENT_CLOSE_FORCE: u32 = 6;
pub const EVENT_CLOSE_FORWARD: u32 = 7;
pub const EVENT_FINISH: u32 = 9;
pub const EVENT_PIPE_MESSAGE: u32 = 10;
pub const EVENT_INCOMING: u32 = 15;
pub const EVENT_SHUTDOWN: u32 = 16;
pub const EVENT_COMMAND_REQUEST: u32 = 17;
pub const EVENT_COMMAND_RESPONSE: u32 = 18;
pub const EVENT_SHUTDOWN_SIGNAL: u32 = 19;
