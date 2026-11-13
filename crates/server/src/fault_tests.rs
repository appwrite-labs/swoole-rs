//! Captured native effects must cut every operation trace at the exact failing call.
use crate::{Effect, Host, NativeFault, ServerState, ops::*, run, script::Script};

struct Failing {
    script: Script,
    at: usize,
}
impl Host for Failing {
    fn op(&mut self, op: u32, arg: u64) -> Effect<u64> {
        if self.script.log.len() == self.at {
            self.script.log.push((op, arg));
            Err(NativeFault(1))
        } else {
            self.script.op(op, arg)
        }
    }
    fn state(&mut self) -> Effect<&ServerState> {
        self.script.state()
    }
}

#[test]
fn every_captured_fault_stops_the_remaining_effect_trace() {
    type Case = (u32, u64, &'static [(u32, u64)]);
    let cases: &[Case] = &[
        (
            RUN_DESTROY,
            0,
            &[
                (OP_MODE, 2),
                (OP_FIELD, 2),
                (OP_PORT_COUNT, 2),
                (OP_IS_STARTED, 1),
            ],
        ),
        (
            RUN_CREATE,
            0,
            &[
                (OP_MODE, 2),
                (OP_INIT_LOCAL_ADDRS, 1),
                (OP_ALLOC_SESSIONS, 1),
                (OP_ALLOC_WORKERS, 1),
                (OP_ALLOC_PORT_GS, 1),
                (OP_CREATE_FACTORY, 1),
            ],
        ),
        (
            RUN_START_MASTER_THREAD,
            0,
            &[
                (OP_MODE, 2),
                (OP_MASTER_TIMER, 1),
                (OP_HAS_PIPE_COMMAND, 1),
                (OP_TIMER_DETACHED, 1),
            ],
        ),
        (RUN_START_CHECK, 0, &[(OP_FIELD, 1), (OP_PORT_COUNT, 2)]),
    ];
    let mut boundaries = 0;
    for &(operation, arg, facts) in cases {
        let mut baseline = Script::with(facts);
        assert!(run(&mut baseline, operation, arg).is_ok());
        assert_ne!(baseline.log, []);
        for at in 0..baseline.log.len() {
            let mut failing = Failing {
                script: Script::with(facts),
                at,
            };
            assert_eq!(run(&mut failing, operation, arg), Err(NativeFault(1)));
            assert_eq!(failing.script.log, baseline.log[..=at]);
            boundaries += 1;
        }
    }
    assert!(
        boundaries > 70,
        "exercise complete multi-effect paths: {boundaries}"
    );
}
