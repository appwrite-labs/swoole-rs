//! `putenv` for Swoole's threads (ZTS builds, `SW_THREAD`): PHP's own keeps
//! its strings per request, so a thread exiting would leave `environ`
//! pointing at freed memory. These strings live for the process, one per
//! name, under PHP's environment lock.
use std::{
    collections::HashMap,
    ffi::CString,
    sync::{LazyLock, Mutex, PoisonError},
};
use swoole_rs_zend::{Call, sys::swr_zend_runtime_env_lock};

swoole_rs_zend::handlers! {
    zif_swoole_putenv => putenv;
}

/// Each name's latest `NAME=value` (or bare `NAME`) string.
static ENVIRON: LazyLock<Mutex<Table>> = LazyLock::new(Mutex::default);

type Table = HashMap<Vec<u8>, CString>;

/// `putenv(string $assignment): bool`.
fn putenv(call: &Call) {
    let Ok(setting) = call.arity(1, 1).and_then(|()| call.string(1, b"")) else {
        return;
    };
    if setting.first().is_none_or(|&first| first == b'=') {
        call.argument_value_error(1, c"must have a valid syntax");
        return;
    }
    // as C reads it: up to the first NUL
    let text = setting.split(|&b| b == 0).next().unwrap_or_default();
    let assignment = text.iter().position(|&b| b == b'=');
    let key = assignment
        .and_then(|end| setting.get(..end))
        .unwrap_or(setting);
    let stored = CString::new(text).unwrap_or_default();

    // SAFETY: PHP's environment lock, released below.
    unsafe { swr_zend_runtime_env_lock(true) };
    let mut environ = ENVIRON.lock().unwrap_or_else(PoisonError::into_inner);
    let set = store(&mut environ, key, stored, assignment.is_none());
    if set && key.eq_ignore_ascii_case(b"TZ") {
        // TZ changed; under the environment lock.
        tzset();
    }
    drop(environ);
    // SAFETY: taken above.
    unsafe { swr_zend_runtime_env_lock(false) };
    call.set_bool(set);
}

/// Point the process environment at `stored` for `key` (or unset `key`), keeping
/// `stored` in `environ`. libc holds the pointer it is given, so the name's previous
/// string is freed only once libc has let go of it.
fn store(environ: &mut Table, key: &[u8], stored: CString, unset: bool) -> bool {
    let pointer = stored.as_ptr().cast_mut();
    let previous = environ.insert(key.to_vec(), stored);
    if unset {
        // SAFETY: a NUL-terminated name.
        unsafe { libc::unsetenv(pointer) };
    }
    // SAFETY: the string stays in `environ` until this name is set again.
    let set = unset || unsafe { libc::putenv(pointer) } == 0;
    if !set {
        // libc took nothing: it still holds the previous string, if there was one.
        match previous {
            Some(previous) => environ.insert(key.to_vec(), previous),
            None => environ.remove(key),
        };
    }
    set
}

unsafe extern "C" {
    /// libc's `tzset`: no arguments, and it only reads the environment.
    safe fn tzset();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::{CStr, c_char};

    unsafe extern "C" {
        static environ: *const *const c_char;
    }

    /// How many entries of the process environment are for `name`.
    fn entries(name: &[u8]) -> usize {
        let (mut count, mut at) = (0, unsafe { environ });
        while let Some(entry) = unsafe { at.as_ref() }.filter(|entry| !entry.is_null()) {
            let entry = unsafe { CStr::from_ptr(*entry) }.to_bytes();
            let rest = entry.strip_prefix(name);
            count += usize::from(rest.is_some_and(|rest| rest.first() == Some(&b'=')));
            at = at.wrapping_add(1);
        }
        count
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no putenv")]
    fn a_name_set_again_keeps_one_entry_then_unsets() {
        let name = b"SWR_PUTENV_TEST";
        let mut map = Table::new();
        for value in [
            "a-first-value-too-long-for-the-second-to-reuse-0123456789",
            "second",
            "third",
        ] {
            let stored = CString::new(format!("SWR_PUTENV_TEST={value}")).unwrap();
            assert!(store(&mut map, name, stored, false), "putenv {value}");
            assert_eq!(entries(name), 1, "after {value}");
            assert_eq!(
                std::env::var("SWR_PUTENV_TEST").as_deref(),
                Ok(value),
                "reads {value}"
            );
        }
        assert!(
            store(
                &mut map,
                name,
                CString::new("SWR_PUTENV_TEST").unwrap(),
                true
            ),
            "unset"
        );
        assert_eq!(entries(name), 0, "after the unset");
    }
}
