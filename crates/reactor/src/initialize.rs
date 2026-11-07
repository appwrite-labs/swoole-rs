//! Reactor construction policy. Native creates backend objects, observes facts,
//! installs Native functions and executes hooks only after each reply returns.
#[repr(C)]
#[derive(Default)]
pub struct ReactorInitState {
    pub phase: u32,
}
#[repr(C)]
pub struct ReactorBackendPlan {
    pub selected: i32,
    pub observe_kqueue: u32,
    pub factory: u32,
}
/// Preserve requested type verbatim; only AUTO chooses a compiled backend.
/// Caps epoll1/kqueue2. Ask Native for runtime enablement only when required.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_reactor_backend_type(
    requested: i32,
    caps: u32,
    known: u32,
    enabled: u32,
) -> ReactorBackendPlan {
    let selected = if requested != 0 {
        requested
    } else if caps & 1 != 0 {
        1
    } else if caps & 2 != 0 && known == 0 {
        return ReactorBackendPlan {
            selected: 0,
            observe_kqueue: 1,
            factory: 0,
        };
    } else if caps & 2 != 0 && enabled != 0 {
        2
    } else {
        3
    };
    ReactorBackendPlan {
        selected,
        observe_kqueue: 0,
        factory: if selected == 1 && caps & 1 != 0 {
            1
        } else if selected == 2 && caps & 2 != 0 {
            2
        } else {
            3
        },
    }
}
/// Actions0done,1observe ready,2initialize fields,3observe create hook,4call hook,
///5install defer-end,6install defer-exit,7install idle,8observe main thread,
///9install signals,10install try-exit,11observe trim main thread,12install trim,
///13install default-exit. Ready/hook/main facts supplied only after observation.
/// # Safety
/// State is zero initialized, aligned/live/exclusive and disjoint from Native
/// objects. No reference or callback survives return. All effects occur outside
/// Rust, and facts are reobserved at the original ordered observation points.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_init_next(
    state: *mut ReactorInitState,
    ready: u32,
    hook: u32,
    main: u32,
    trim_compiled: u32,
) -> i32 {
    // SAFETY: `state` is null or live, aligned and exclusively lent for this call.
    let Some(state) = (unsafe { state.as_mut() }) else {
        return -1;
    };
    loop {
        if state.phase > 14 {
            return -1;
        }
        let phase = state.phase;
        state.phase += 1;
        match phase {
            0 => return 1,
            1 if ready == 0 => {
                state.phase = 14;
                return 0;
            }
            1 => return 2,
            2 => return 3,
            3 if hook != 0 => return 4,
            3 => {}
            4 => return 5,
            5 => return 6,
            6 => return 7,
            7 => return 8,
            8 if main != 0 => return 9,
            8 => {}
            9 => return 10,
            10 if trim_compiled != 0 => return 11,
            10 => {
                state.phase = 12;
            }
            11 if main != 0 => return 12,
            11 => {}
            12 => return 13,
            13 | 14 => {
                state.phase = 14;
                return 0;
            }
            _ => {
                state.phase = phase;
                return -1;
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn initialize_positive_backend_and_complete_order() {
        assert_eq!(std::mem::size_of::<ReactorBackendPlan>(), 12);
        assert_eq!(std::mem::align_of::<ReactorBackendPlan>(), 4);
        assert_eq!(std::mem::offset_of!(ReactorBackendPlan, observe_kqueue), 4);
        assert_eq!(std::mem::size_of::<ReactorInitState>(), 4);
        for caps in 0..4 {
            assert_eq!(
                swr_v1_reactor_backend_type(0, caps, 0, 0).observe_kqueue,
                u32::from(caps & 1 == 0 && caps & 2 != 0)
            );
            for enabled in 0..2 {
                assert_eq!(
                    swr_v1_reactor_backend_type(0, caps, 1, enabled).selected,
                    if caps & 1 != 0 {
                        1
                    } else if caps & 2 != 0 && enabled != 0 {
                        2
                    } else {
                        3
                    }
                );
                for explicit in [1, 2, 3] {
                    let plan = swr_v1_reactor_backend_type(explicit, caps, 0, enabled);
                    assert_eq!(plan.selected, explicit);
                    assert_eq!(
                        plan.factory,
                        if explicit == 1 && caps & 1 != 0 {
                            1
                        } else if explicit == 2 && caps & 2 != 0 {
                            2
                        } else {
                            3
                        }
                    );
                }
            }
        }
        for hook in 0..2 {
            for main in 0..2 {
                for trim in 0..2 {
                    let mut state = ReactorInitState::default();
                    let mut actions = Vec::new();
                    loop {
                        let action = unsafe {
                            swr_v1_reactor_init_next(&raw mut state, 1, hook, main, trim)
                        };
                        if action == 0 {
                            break;
                        }
                        actions.push(action);
                    }
                    let mut expected = vec![1, 2, 3];
                    if hook != 0 {
                        expected.push(4);
                    }
                    expected.extend([5, 6, 7, 8]);
                    if main != 0 {
                        expected.push(9);
                    }
                    expected.push(10);
                    if trim != 0 {
                        expected.push(11);
                        if main != 0 {
                            expected.push(12);
                        }
                    }
                    expected.push(13);
                    assert_eq!(actions, expected);
                }
            }
        }
    }

    /// What reactor.cc's `Reactor::Reactor` did, as the former initialize gate traced it:
    /// backend open `factory*100 + max_event`, hook observe 20 and call 21, main-thread
    /// observe 30, end callback `40 + priority`, exit condition `50 + key`.
    struct Constructed {
        type_: i32,
        ready: bool,
        timeout_msec: i32,
        fields: bool,
        trace: Vec<u32>,
        ends: Vec<u32>,
        exits: Vec<u32>,
    }

    fn construct(
        requested: i32,
        caps: u32,
        trim: u32,
        kqueue: u32,
        hook: u32,
        mains: [u32; 2],
    ) -> Constructed {
        let mut plan = swr_v1_reactor_backend_type(requested, caps, 0, 0);
        if caps & 2 != 0 && plan.observe_kqueue != 0 {
            plan = swr_v1_reactor_backend_type(requested, caps, 1, kqueue);
        }
        let mut built = Constructed {
            type_: plan.selected,
            ready: true,
            timeout_msec: 0,
            fields: false,
            trace: vec![plan.factory * 100 + 7],
            ends: Vec::new(),
            exits: Vec::new(),
        };
        let (mut state, mut hook_fact, mut main, mut observed) =
            (ReactorInitState::default(), 0, 0, 0);
        loop {
            let action =
                unsafe { swr_v1_reactor_init_next(&raw mut state, 1, hook_fact, main, trim) };
            let (end, exit) = match action {
                0 => return built,
                1 => (None, None),
                2 => {
                    (built.timeout_msec, built.fields) = (-1, true);
                    (None, None)
                }
                3 => {
                    built.trace.push(20);
                    hook_fact = hook;
                    (None, None)
                }
                4 => {
                    built.trace.push(21);
                    (None, None)
                }
                8 | 11 => {
                    built.trace.push(30);
                    main = mains[observed];
                    observed += 1;
                    (None, None)
                }
                5 => (Some(1), None),
                6 => (None, Some(1)),
                7 => (Some(2), None),
                9 => (Some(3), None),
                10 => (Some(4), None),
                12 => (Some(5), None),
                13 => (None, Some(999)),
                other => panic!("action {other}"),
            };
            if let Some(key) = end {
                built.trace.push(40 + key);
                built.ends.push(key);
            }
            if let Some(key) = exit {
                built.trace.push(50 + key);
                built.exits.push(key);
            }
        }
    }

    /// The former initialize gate: every requested type, kqueue enablement, create hook
    /// and main-thread pair, under each epoll/kqueue capability and malloc-trim profile.
    #[test]
    fn former_initialize_gate_constructor_order() {
        for caps in 0..4 {
            for trim in 0..2 {
                for requested in 0..4 {
                    for kqueue in 0..2 {
                        for hook in 0..2 {
                            for mains in 0..4 {
                                let (first, second) = (mains & 1, (mains >> 1) & 1);
                                let r =
                                    construct(requested, caps, trim, kqueue, hook, [first, second]);
                                let chosen = match requested {
                                    0 if caps & 1 != 0 => 1,
                                    0 if caps & 2 != 0 && kqueue != 0 => 2,
                                    0 => 3,
                                    other => other,
                                };
                                assert_eq!(r.type_, chosen);
                                assert!(r.ready && r.fields);
                                assert_eq!(r.timeout_msec, -1);
                                let mut expected = vec![match chosen {
                                    1 if caps & 1 != 0 => 107,
                                    2 if caps & 2 != 0 => 207,
                                    _ => 307,
                                }];
                                expected.push(20);
                                if hook != 0 {
                                    expected.push(21);
                                }
                                expected.extend([41, 51, 42, 30]);
                                if first != 0 {
                                    expected.push(43);
                                }
                                expected.push(44);
                                if trim != 0 {
                                    expected.push(30);
                                    if second != 0 {
                                        expected.push(45);
                                    }
                                }
                                expected.push(1049);
                                assert_eq!(r.trace, expected);
                                assert!([1, 2, 4].iter().all(|key| r.ends.contains(key)));
                                assert!(r.exits.contains(&1) && r.exits.contains(&999));
                                assert_eq!(r.ends.contains(&3), first != 0);
                                // The installed exits: defer (no queue yet) and default (no events).
                                let defer_exit =
                                    crate::swr_v1_reactor_builtin_step(4, 0, 0, 0).action;
                                let default_exit =
                                    crate::swr_v1_reactor_builtin_step(5, 0, 0, 0).action;
                                assert_eq!((defer_exit, default_exit), (9, 9));
                            }
                        }
                    }
                }
            }
        }
    }
}
