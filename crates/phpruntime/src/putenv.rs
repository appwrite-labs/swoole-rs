//! `putenv` for Swoole's threads (ZTS builds, `SW_THREAD`): its strings live
//! for the process, one per name, under PHP's environment lock.
use std::ffi::CStr;
use std::sync::{LazyLock, Mutex, PoisonError};
use swoole_rs_hooks::putenv::{Assignment, Table};
use swoole_rs_zend::Call;

swoole_rs_zend::handlers! {
    zif_swoole_putenv => putenv;
}

/// Each name's latest `NAME=value` (or bare `NAME`) string.
static ENVIRON: LazyLock<Mutex<Table>> = LazyLock::new(Mutex::default);

/// `putenv(string $assignment): bool`.
fn putenv(call: &Call) {
    let Ok(setting) = call.arity(1, 1).and_then(|()| call.string(1, b"")) else {
        return;
    };
    let Some(assignment) = Assignment::parse(setting) else {
        call.argument_value_error(1, c"must have a valid syntax");
        return;
    };
    let timezone = assignment.key.eq_ignore_ascii_case(b"TZ");
    let lock = call.php().env_lock();
    let mut environ = ENVIRON.lock().unwrap_or_else(PoisonError::into_inner);
    let set = assignment.store(&mut environ, update_environment);
    if set && timezone {
        // TZ changed; under the environment lock.
        tzset();
    }
    drop(environ);
    drop(lock);
    call.set_bool(set);
}

fn update_environment(stored: &CStr, unset: bool) -> bool {
    if unset {
        unsetenv(stored.into());
        return true;
    }
    // SAFETY: the process table retains this string on success until libc releases it.
    unsafe { libc::putenv(stored.as_ptr().cast_mut()) == 0 }
}

unsafe extern "C" {
    /// libc's `tzset`: no arguments, and it only reads the environment.
    safe fn tzset();
    /// libc's `unsetenv`: it reads the name and keeps nothing.
    safe fn unsetenv(name: swoole_rs_corebase::sys::Text<'_>) -> std::ffi::c_int;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::c_char;

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
            let text = format!("SWR_PUTENV_TEST={value}");
            assert!(
                Assignment::parse(text.as_bytes())
                    .unwrap()
                    .store(&mut map, update_environment),
                "putenv {value}"
            );
            assert_eq!(entries(name), 1, "after {value}");
            assert_eq!(
                std::env::var("SWR_PUTENV_TEST").as_deref(),
                Ok(value),
                "reads {value}"
            );
        }
        assert!(
            Assignment::parse(name)
                .unwrap()
                .store(&mut map, update_environment),
            "unset"
        );
        assert_eq!(entries(name), 0, "after the unset");
    }
}
