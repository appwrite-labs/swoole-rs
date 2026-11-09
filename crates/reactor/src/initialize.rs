//! The reactor backend a construction selects; native/reactor/reactor.cc opens it.
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
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn backend_type_selects_a_compiled_backend() {
        assert_eq!(std::mem::size_of::<ReactorBackendPlan>(), 12);
        assert_eq!(std::mem::align_of::<ReactorBackendPlan>(), 4);
        assert_eq!(std::mem::offset_of!(ReactorBackendPlan, observe_kqueue), 4);
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
    }
}
