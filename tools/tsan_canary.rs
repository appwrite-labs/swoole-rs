//! tools/tsan.py's planted race: when the library loads, a thread it starts and the loading
//! thread both write one static with nothing ordering the writes, so ThreadSanitizer must
//! report it. Test-only; nothing else builds it.

use std::thread;

static mut VALUE: usize = 0;

extern "C" fn plant() {
    let racer = thread::spawn(bump);
    bump();
    racer.join().ok();
}

fn bump() {
    let value = &raw mut VALUE;
    // SAFETY: none; the unsynchronised write is the race the run must report.
    unsafe { value.write_volatile(value.read_volatile() + 1) };
}

#[used]
#[unsafe(link_section = ".init_array")]
static PLANT: extern "C" fn() = plant;
