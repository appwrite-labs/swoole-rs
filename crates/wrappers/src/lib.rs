//! `swoole_event_init`'s admission (`native/reactor/reactor.cc` runs its effects).
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

#[repr(C)]
#[derive(Default)]
pub struct EventInitPlan {
    pub accepted: u32,
    pub wait_exit: u32,
    pub initializers: [u32; 3],
}

/// Copied readiness/flag facts only. Native allocation, initializer calls and
/// final `SwooleTG` publication occur outside Rust using the original local owner.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_wrappers_event_init(
    flags: u32,
    wait_exit_mask: u32,
    ready: u32,
) -> EventInitPlan {
    if ready == 0 {
        return EventInitPlan::default();
    }
    EventInitPlan {
        accepted: 1,
        wait_exit: u32::from(flags & wait_exit_mask != 0),
        initializers: [1, 2, 3],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Against upstream v6.2.4's `src/wrapper/event.cc` expressions.
    #[test]
    fn event_init_matches_the_native_expressions() {
        let refused = swr_v1_wrappers_event_init(u32::MAX, u32::MAX, 0);
        assert_eq!(
            (refused.accepted, refused.wait_exit, refused.initializers),
            (0, 0, [0; 3])
        );
        for flags in [0, 1, 4, 6] {
            let plan = swr_v1_wrappers_event_init(flags, 4, 1);
            assert_eq!(plan.accepted, 1);
            assert_eq!(plan.wait_exit, u32::from(flags & 4 != 0), "{flags}");
            assert_eq!(plan.initializers, [1, 2, 3]);
        }
    }
}
