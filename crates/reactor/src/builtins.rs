//! Idle/exit policy only. Native retains the `std::function` payloads and their calls;
//! the deferred queue is `deferred.rs`.
#[repr(C)]
pub struct ReactorBuiltinReply {
    pub phase: u32,
    pub action: i32,
}
/// Roles: 2 idle/future, 3 try-exit, 4 defer-exit, 5 default-exit.
/// Actions: 0 done/false, 6 invoke-task, 7 query-exit, 8 publish-stopped, 9 true; -1 invalid.
/// Flag 1 captures initial manager/task/wait presence; flag 2 is the Native exit
/// result after action 7. Native executes every action after return and stops
/// immediately after append/delete/invoke/stop, with no post-delete field read.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_reactor_builtin_step(
    role: u32,
    phase: u32,
    flags: u32,
    event_count: u64,
) -> ReactorBuiltinReply {
    let reply = |phase, action| ReactorBuiltinReply { phase, action };
    match (role, phase) {
        (2, 0) => reply(1, if flags & 1 != 0 { 6 } else { 0 }),
        (2, 1) => reply(1, 0),
        (3, 0) => reply(
            if flags & 1 != 0 { 1 } else { 2 },
            if flags & 1 != 0 { 7 } else { 0 },
        ),
        (3, 1) => reply(2, if flags & 2 != 0 { 8 } else { 0 }),
        (3, 2) => reply(2, 0),
        (4, 0) => reply(1, if flags & 1 == 0 { 9 } else { 0 }),
        (5, 0) => reply(1, if event_count == 0 { 9 } else { 0 }),
        _ => reply(phase, -1),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn builtins_positive_idle_and_exit_order() {
        assert_eq!(std::mem::size_of::<ReactorBuiltinReply>(), 8);
        assert_eq!(std::mem::offset_of!(ReactorBuiltinReply, action), 4);
        for present in 0..2 {
            assert_eq!(
                swr_v1_reactor_builtin_step(2, 0, present, 0).action,
                if present != 0 { 6 } else { 0 }
            );
            assert_eq!(
                swr_v1_reactor_builtin_step(4, 0, present, 0).action,
                if present == 0 { 9 } else { 0 }
            );
            let exit = swr_v1_reactor_builtin_step(3, 0, present, 0);
            if present == 0 {
                assert_eq!(exit.action, 0);
            } else {
                assert_eq!(exit.action, 7);
                for accepted in 0..2 {
                    assert_eq!(
                        swr_v1_reactor_builtin_step(3, exit.phase, accepted << 1, 0).action,
                        if accepted != 0 { 8 } else { 0 }
                    );
                }
            }
        }
        for count in [0, 1, u64::MAX] {
            assert_eq!(
                swr_v1_reactor_builtin_step(5, 0, 0, count).action,
                if count == 0 { 9 } else { 0 }
            );
        }
    }
}
