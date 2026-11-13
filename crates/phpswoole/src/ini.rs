//! Persistent module INI definitions. Zend owns the registered entries; their
//! callback arguments refer to the native module's real NTS globals or ZTS id.
use crate::module::INI_OFFSETS;
use std::{
    ffi::{CStr, c_int},
    ptr,
};
use swoole_rs_zend::{
    Minit,
    bailout::{self, Bailout},
    sys::{
        OnUpdateBool, OnUpdateLong, swr_zend_try_module_ini, zend_ini_boolean_displayer_cb,
        zend_ini_entry_def,
    },
};

#[expect(
    clippy::cast_possible_truncation,
    reason = "const assertions bound literal INI names and defaults to Zend's field widths"
)]
const fn entry(
    name: &CStr,
    default: &CStr,
    offset: usize,
    system: bool,
    boolean: bool,
) -> zend_ini_entry_def {
    assert!(
        name.to_bytes().len() <= u16::MAX as usize,
        "INI name must fit Zend length"
    );
    assert!(
        default.to_bytes().len() <= u32::MAX as usize,
        "INI default must fit Zend length"
    );
    zend_ini_entry_def {
        name: name.as_ptr(),
        on_modify: if boolean {
            Some(OnUpdateBool)
        } else {
            Some(OnUpdateLong)
        },
        mh_arg1: ptr::without_provenance_mut(offset),
        mh_arg2: ptr::null_mut(),
        mh_arg3: ptr::null_mut(),
        value: default.as_ptr(),
        displayer: if boolean {
            Some(zend_ini_boolean_displayer_cb)
        } else {
            None
        },
        value_length: default.to_bytes().len() as u32,
        name_length: name.to_bytes().len() as u16,
        modifiable: if system { 4 } else { 7 },
    }
}

// The definitions, less each callback's globals argument, which MINIT knows.
const ENTRIES: [zend_ini_entry_def; 11] = [
    entry(c"swoole.enable_library", c"On", INI_OFFSETS[0], false, true),
    entry(
        c"swoole.enable_fiber_mock",
        c"Off",
        INI_OFFSETS[1],
        false,
        true,
    ),
    entry(
        c"swoole.enable_preemptive_scheduler",
        c"Off",
        INI_OFFSETS[2],
        false,
        true,
    ),
    entry(c"swoole.display_errors", c"On", INI_OFFSETS[3], false, true),
    entry(c"swoole.use_shortname", c"On", INI_OFFSETS[4], true, true),
    entry(
        c"swoole.socket_buffer_size",
        c"8388608",
        INI_OFFSETS[5],
        false,
        false,
    ),
    entry(
        c"swoole.blocking_detection",
        c"Off",
        INI_OFFSETS[6],
        true,
        true,
    ),
    entry(
        c"swoole.blocking_threshold",
        c"100000",
        INI_OFFSETS[7],
        true,
        false,
    ),
    entry(c"swoole.profile", c"Off", INI_OFFSETS[8], true, true),
    entry(c"swoole.leak_detection", c"Off", INI_OFFSETS[9], true, true),
    zend_ini_entry_def {
        name: ptr::null(),
        on_modify: None,
        mh_arg1: ptr::null_mut(),
        mh_arg2: ptr::null_mut(),
        mh_arg3: ptr::null_mut(),
        value: ptr::null(),
        displayer: None,
        value_length: 0,
        name_length: 0,
        modifiable: 0,
    },
];

/// Register the module's real INI entries, including native Zend update handlers,
/// after making the module globals they write (`swoole_globals`, or its id under ZTS).
pub(crate) fn register(minit: &Minit, kind: c_int) {
    let globals = crate::module::globals_init();
    // Zend copies each definition into an entry of its own, so these needn't outlive
    // the call (a leaked copy was a LeakSanitizer report in every process, #3650).
    let mut entries = ENTRIES;
    // All but the terminator.
    for entry in &mut entries[..10] {
        entry.mh_arg2 = globals;
    }
    // SAFETY: MINIT, as the token says; the terminated definitions, which Zend
    // copies, and the process-lifetime globals their handlers write; the native
    // leaf traps Zend allocation/update-handler bailouts below this frame.
    let bailed = unsafe { swr_zend_try_module_ini(entries.as_ptr(), minit.module(), kind) };
    bailout::defer(if bailed == 0 { Ok(()) } else { Err(Bailout) });
    bailout::finish();
}
