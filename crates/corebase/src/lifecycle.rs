// SPDX-License-Identifier: Apache-2.0
//! Stack-owned lifecycle sequencing. Native effects run only after this returns.
#[repr(C)]
#[derive(Default)]
pub struct LifecycleState {
    phase: u32,
    index: u32,
}

const INVALID: u32 = u32::MAX;

#[repr(u32)]
#[derive(Clone, Copy)]
enum Action {
    Next = 0,
    ObserveBuffer = 1,
    AllocateBuffer = 2,
    BlockSignals = 3,
    PublishMain = 4,
    ObserveTimer = 5,
    FreeTimer = 6,
    ObserveReactor = 7,
    FreeReactor = 8,
    DeleteBuffer = 9,
    ClearSignals = 12,
    CleanThread = 13,
    ObserveLogger = 14,
    CloseLogger = 15,
    DeleteLogger = 16,
    DeletePool = 17,
    ResetGlobal = 18,
    ObserveCoroutine = 19,
    RejectCoroutine = 20,
    ObserveAsync = 21,
    RejectAsync = 22,
    Fork = 23,
    WarnFork = 24,
    SleepRetry = 25,
    ReplacePool = 26,
    ReopenLogger = 27,
    FreeForkReactor = 28,
    ObserveAfterFork = 29,
    CallAfterFork = 30,
    ExecCallback = 31,
    ExitChild = 32,
    FailedExec = 33,
    ObserveTaskByte = 47,
    WarnTaskAbsolute = 48,
    AccessTask = 49,
    MkdirTask = 50,
    WarnTaskCreate = 51,
    PublishTask = 52,
    ObserveTaskLength = 53,
    WarnTaskLong = 54,
    RejectTask = 55,
    AcceptTask = 56,
}

#[derive(Clone, Copy)]
enum Operation {
    ThreadInit,
    ThreadClean,
    Clean,
    Fork,
    ForkExec,
    TaskTmp,
}

/// Return the next native action, or zero when finished. Observations are copied
/// booleans produced by the requested observation action, never native pointers.
/// # Safety
/// State is writable, initially zero, and exclusively owned for this call. Keep
/// it unchanged between calls except through this function. No borrow survives
/// return. Operation 1 takes main-thread bool, 2 and 3 ignore argument.
/// Operations 4/5 sequence `fork/fork_exec` with pinned native flags as argument. After Fork, observed is 0=failure, 1=child,
/// 2=parent; other requested observations are copied booleans. Keep operation
/// and argument fixed. Stop on zero/INVALID; execute each action before next.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_corebase_lifecycle_next(
    operation: u32,
    state: *mut LifecycleState,
    observed: u64,
    argument: u32,
) -> u32 {
    let operation = match operation {
        1 => Operation::ThreadInit,
        2 => Operation::ThreadClean,
        3 => Operation::Clean,
        4 => Operation::Fork,
        5 => Operation::ForkExec,
        _ => return INVALID,
    };
    // SAFETY: forwards the caller's exclusive, initially zeroed state unchanged.
    unsafe { next(operation, state, observed, argument) }
}

/// Task-directory acceptance/publication sequencing, using copied native facts.
/// # Safety
/// State is exclusively writable and initially zero. Execute each requested
/// effect after return. Observations are first byte from native `string::at(0)`,
/// nonnegative access / successful mkdir booleans, then published string length.
/// Preserve cursor/limit. Stop on `AcceptTask`, `RejectTask` or INVALID. Native owns
/// lock, strings, formatting, publication, IO and exceptions; no pointer retained.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_corebase_task_tmp_next(
    state: *mut LifecycleState,
    observed: u64,
    limit: u32,
) -> u32 {
    // SAFETY: forwards the caller's exclusive, initially zeroed state unchanged.
    unsafe { next(Operation::TaskTmp, state, observed, limit) }
}

unsafe fn next(
    operation: Operation,
    state: *mut LifecycleState,
    observed: u64,
    argument: u32,
) -> u32 {
    // SAFETY: the caller passes null or exclusively writable, initialized state.
    let Some(state) = (unsafe { state.as_mut() }) else {
        return INVALID;
    };
    loop {
        let (next, action) = match (operation, state.phase) {
            // Preserve at(0) exception, access/mkdir short circuit, and publication
            // before the length rejection. Each observation follows its effect.
            (Operation::TaskTmp, 0) => (1, Action::ObserveTaskByte),
            (Operation::TaskTmp, 1) if observed != u64::from(b'/') => (7, Action::WarnTaskAbsolute),
            (Operation::TaskTmp, 1) => (2, Action::AccessTask),
            (Operation::TaskTmp, 2) if observed == 0 => (3, Action::MkdirTask),
            (Operation::TaskTmp, 2) => (4, Action::Next),
            (Operation::TaskTmp, 3) if observed == 0 => (7, Action::WarnTaskCreate),
            (Operation::TaskTmp, 3) => (4, Action::Next),
            (Operation::TaskTmp, 4) => (5, Action::PublishTask),
            (Operation::TaskTmp, 5) => (6, Action::ObserveTaskLength),
            (Operation::TaskTmp, 6) if observed >= u64::from(argument) => (7, Action::WarnTaskLong),
            (Operation::TaskTmp, 6) => (8, Action::Next),
            (Operation::TaskTmp, 7) => return Action::RejectTask as u32,
            (Operation::TaskTmp, 8) => return Action::AcceptTask as u32,
            // thread_init: allocate buffer, block only non-main, publish flag.
            (Operation::ThreadInit, 0) => (1, Action::ObserveBuffer), // observe buffer
            (Operation::ThreadInit, 1) if observed == 0 => (2, Action::AllocateBuffer), // allocate buffer
            (Operation::ThreadInit, 1) => (2, Action::Next),
            (Operation::ThreadInit, 2) if argument == 0 => (3, Action::BlockSignals), // block signals
            (Operation::ThreadInit, 2) => (3, Action::Next),
            (Operation::ThreadInit, 3) => (4, Action::PublishMain), // publish main-thread flag
            (Operation::ThreadInit, 4) => return 0,
            // thread_clean: observations must follow each prior destruction.
            (Operation::ThreadClean, 0) => (1, Action::ObserveTimer), // observe timer
            (Operation::ThreadClean, 1) if observed != 0 => (2, Action::FreeTimer), // native timer free
            (Operation::ThreadClean, 1) => (2, Action::Next),
            (Operation::ThreadClean, 2) => (3, Action::ObserveReactor), // observe reactor
            (Operation::ThreadClean, 3) if observed != 0 => (4, Action::FreeReactor), // native reactor free
            (Operation::ThreadClean, 3) => (4, Action::Next),
            (Operation::ThreadClean, 4) => (5, Action::ObserveBuffer), // observe buffer
            (Operation::ThreadClean, 5) if observed != 0 => (6, Action::DeleteBuffer), // delete, then null buffer
            (Operation::ThreadClean, 5) => (6, Action::Next),
            (Operation::ThreadClean, 6) => return 0,
            // clean: release the hook lists before all other cleanup.
            (Operation::Clean, 0) => {
                crate::hook::clear();
                (4, Action::ClearSignals)
            }
            (Operation::Clean, 4) => (5, Action::CleanThread), // native thread cleanup
            (Operation::Clean, 5) => (6, Action::ObserveLogger), // observe logger after thread cleanup
            (Operation::Clean, 6) if observed != 0 => (7, Action::CloseLogger), // close logger
            (Operation::Clean, 6) => (7, Action::Next),
            (Operation::Clean, 7) => (8, Action::DeleteLogger), // delete current native logger
            (Operation::Clean, 8) => (9, Action::DeletePool),   // delete current native memory pool
            (Operation::Clean, 9) => (10, Action::ResetGlobal), // reset native Global last
            (Operation::Clean, 10) => return 0,
            // Native fork flags are frozen by static_assert in the real caller.
            // observed is 0=failure, 1=child, 2=parent after the Fork action.
            (Operation::Fork, 0) if argument & 2 == 0 => (1, Action::ObserveCoroutine),
            (Operation::Fork, 0) => (4, Action::Next),
            (Operation::Fork, 1) if observed != 0 => (2, Action::RejectCoroutine),
            (Operation::Fork, 1) => (2, Action::Next),
            // A custom fatal handler may return; query async only afterward.
            (Operation::Fork, 2) => (3, Action::ObserveAsync),
            (Operation::Fork, 3) if observed != 0 => (4, Action::RejectAsync),
            (Operation::Fork, 3) => (4, Action::Next),
            (Operation::Fork, 4) if argument & 8 != 0 => return 0,
            (Operation::Fork, 4) => (5, Action::Fork),
            (Operation::Fork, 5) if observed == 0 => (6, Action::WarnFork),
            (Operation::Fork, 5) if observed == 2 => return 0,
            (Operation::Fork, 5) if observed == 1 && argument & 4 != 0 => return 0,
            (Operation::Fork, 5) if observed == 1 => (8, Action::Next),
            (Operation::Fork, 5) => return INVALID,
            (Operation::Fork, 6) => (7, Action::SleepRetry),
            (Operation::Fork, 7) => (5, Action::Fork),
            // Child DAEMON already returned before any native timer query.
            (Operation::Fork, 8) => (9, Action::ObserveTimer),
            (Operation::Fork, 9) if observed != 0 => (10, Action::FreeTimer),
            (Operation::Fork, 9) => (10, Action::Next),
            (Operation::Fork, 10) if argument & 2 != 0 => (14, Action::CloseLogger),
            (Operation::Fork, 10) => (11, Action::ReplacePool),
            (Operation::Fork, 11) => (12, Action::ReopenLogger),
            (Operation::Fork, 12) => (13, Action::ObserveReactor),
            (Operation::Fork, 13) if observed != 0 => (14, Action::FreeForkReactor),
            (Operation::Fork, 13) => (14, Action::Next),
            (Operation::Fork, 14) => (15, Action::ClearSignals),
            (Operation::Fork, 15) => (16, Action::ObserveAfterFork),
            (Operation::Fork, 16) if observed != 0 => (17, Action::CallAfterFork),
            (Operation::Fork, 16 | 17) => return 0,
            (Operation::ForkExec, 0) => (1, Action::Fork),
            (Operation::ForkExec, 1) if observed == 0 => (3, Action::FailedExec),
            (Operation::ForkExec, 1) if observed == 1 => (2, Action::ExecCallback),
            (Operation::ForkExec, 1) if observed == 2 => return 0,
            (Operation::ForkExec, 2) => (3, Action::ExitChild),
            (Operation::ForkExec, 3) => return 0,
            _ => return INVALID,
        };
        state.phase = next;
        if action as u32 != 0 {
            return action as u32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trace(first: u8, accessible: bool, created: bool, published: u64) -> Vec<u32> {
        let mut cursor = LifecycleState::default();
        let mut observed = 0;
        let mut actions = Vec::new();
        for _ in 0..12 {
            let action = unsafe { swr_v1_corebase_task_tmp_next(&raw mut cursor, observed, 256) };
            actions.push(action);
            observed = match action {
                47 => u64::from(first),
                49 => u64::from(accessible),
                50 => u64::from(created),
                53 => published,
                55 | 56 => return actions,
                _ => 0,
            };
        }
        panic!("policy did not terminate");
    }

    #[test]
    fn relative_path_rejection_precedes_access_and_publication() {
        assert_eq!(trace(b'a', true, true, 0), [47, 48, 55]);
    }
    #[test]
    fn access_success_bypasses_mkdir_and_observes_after_publication() {
        assert_eq!(trace(b'/', true, false, 255), [47, 49, 52, 53, 56]);
    }
    #[test]
    fn failed_access_requires_successful_mkdir_before_publication() {
        assert_eq!(trace(b'/', false, true, 255), [47, 49, 50, 52, 53, 56]);
        assert_eq!(trace(b'/', false, false, 255), [47, 49, 50, 51, 55]);
    }
    #[test]
    fn length_limit_rejects_only_after_native_publication() {
        assert_eq!(trace(b'/', true, false, 256), [47, 49, 52, 53, 54, 55]);
        assert_eq!(trace(b'/', true, false, u64::MAX), [47, 49, 52, 53, 54, 55]);
    }
}
