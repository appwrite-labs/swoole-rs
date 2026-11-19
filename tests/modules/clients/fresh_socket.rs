//! PHP hosts this fixture so the socket destructor enters the real Zend trap.
use std::cell::Cell;
use std::rc::Rc;
use swoole_rs_phpruntime::co;
use swoole_rs_zend::bailout::{self, Bailout};

#[unsafe(no_mangle)]
pub extern "C" fn fresh_socket_drop_count() -> i32 {
    let drops = Rc::new(Cell::new(0));
    let on_close = Rc::clone(&drops);
    let allocate = || {
        let fresh = co::make(-1, libc::AF_INET, libc::SOCK_STREAM, 0, true)?;
        swoole_rs_hooks::socket_extra::set_close_hook(
            fresh.co(),
            Box::new(move |_| on_close.set(on_close.get() + 1)),
        );
        // A warning handler has left a bailout pending before the socket reaches
        // its PHP object. The early return must run the fresh owner's destructor.
        Bailout.defer();
        bailout::outcome(())?;
        Ok::<_, Bailout>(fresh)
    };
    let bailed = allocate().is_err();
    if bailout::relay_to_c(Ok(())) && bailed {
        drops.get()
    } else {
        -1
    }
}
