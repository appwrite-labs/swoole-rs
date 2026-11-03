//! The module's function table (`swoole_functions[]` in `php_swoole.cc`), in
//! upstream's order, and the short-name aliases MINIT adds.
use crate::{constants::SHORTNAME, functions as f, interfaces as i, module, vm};
use std::ffi::CStr;
use swoole_rs_phpstubs::{ex, stdext, swoole as s, tracer};
use swoole_rs_zend::{
    method_table,
    sys::{self as z, zend_function_entry, zend_internal_arg_info},
};

/// `swr_v1_phpswoole_minit` flag: the build has `SW_STDEXT`.
pub(crate) const STDEXT: u32 = 1 << 11;

// Handlers other crates or C++ define, whichever the build selects.
unsafe extern "C" {
    fn zif_swoole_async_dns_lookup_coro(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_swoole_async_set(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_swoole_coroutine_create(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_swoole_coroutine_defer(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_swoole_coroutine_socketpair(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_swoole_test_kernel_coroutine(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_swoole_client_select(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_swoole_clear_dns_cache(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_swoole_implicit_fn(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_swoole_name_resolver_lookup(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_swoole_name_resolver_add(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_swoole_name_resolver_remove(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_swoole_tracer_leak_detect(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_swoole_tracer_prof_begin(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_swoole_tracer_prof_end(frame: *mut z::zend_execute_data, value: *mut z::zval);
}

method_table! {
    static HEAD = [
        c"swoole_version" => f::zif_swoole_version, &s::arginfo_swoole_version, 0;
        c"swoole_cpu_num" => f::zif_swoole_cpu_num, &s::arginfo_swoole_cpu_num, 0;
        c"swoole_last_error" => f::zif_swoole_last_error, &s::arginfo_swoole_last_error, 0;
        c"swoole_async_dns_lookup_coro" => zif_swoole_async_dns_lookup_coro, &s::arginfo_swoole_async_dns_lookup_coro, 0;
        c"swoole_async_set" => zif_swoole_async_set, &s::arginfo_swoole_async_set, 0;
        c"swoole_coroutine_create" => zif_swoole_coroutine_create, &s::arginfo_swoole_coroutine_create, 0;
        c"swoole_coroutine_defer" => zif_swoole_coroutine_defer, &s::arginfo_swoole_coroutine_defer, 0;
        c"swoole_coroutine_socketpair" => zif_swoole_coroutine_socketpair, &s::arginfo_swoole_coroutine_socketpair, 0;
        c"swoole_test_kernel_coroutine" => zif_swoole_test_kernel_coroutine, &s::arginfo_swoole_test_kernel_coroutine, 0;
        c"swoole_client_select" => zif_swoole_client_select, &s::arginfo_swoole_client_select, 0;
        c"swoole_select" => zif_swoole_client_select, &s::arginfo_swoole_client_select, 0;
        c"swoole_set_process_name" => module::zif_swoole_set_process_name, &s::arginfo_swoole_set_process_name, 0;
        c"swoole_get_local_ip" => i::zif_swoole_get_local_ip, &s::arginfo_swoole_get_local_ip, 0;
        c"swoole_get_local_mac" => i::zif_swoole_get_local_mac, &s::arginfo_swoole_get_local_mac, 0;
        c"swoole_strerror" => f::zif_swoole_strerror, &s::arginfo_swoole_strerror, 0;
        c"swoole_errno" => f::zif_swoole_errno, &s::arginfo_swoole_errno, 0;
        c"swoole_clear_error" => f::zif_swoole_clear_error, &s::arginfo_swoole_clear_error, 0;
        c"swoole_error_log" => f::zif_swoole_error_log, &s::arginfo_swoole_error_log, 0;
        c"swoole_error_log_ex" => f::zif_swoole_error_log_ex, &s::arginfo_swoole_error_log_ex, 0;
        c"swoole_ignore_error" => f::zif_swoole_ignore_error, &s::arginfo_swoole_ignore_error, 0;
        c"swoole_hashcode" => f::zif_swoole_hashcode, &s::arginfo_swoole_hashcode, 0;
        c"swoole_mime_type_add" => f::zif_swoole_mime_type_add, &s::arginfo_swoole_mime_type_add, 0;
        c"swoole_mime_type_set" => f::zif_swoole_mime_type_set, &s::arginfo_swoole_mime_type_set, 0;
        c"swoole_mime_type_delete" => f::zif_swoole_mime_type_delete, &s::arginfo_swoole_mime_type_delete, 0;
        c"swoole_mime_type_get" => f::zif_swoole_mime_type_get, &s::arginfo_swoole_mime_type_get, 0;
        c"swoole_get_mime_type" => f::zif_swoole_mime_type_get, &s::arginfo_swoole_mime_type_get, 0;
        c"swoole_mime_type_exists" => f::zif_swoole_mime_type_exists, &s::arginfo_swoole_mime_type_exists, 0;
        c"swoole_mime_type_list" => f::zif_swoole_mime_type_list, &s::arginfo_swoole_mime_type_list, 0;
        c"swoole_clear_dns_cache" => zif_swoole_clear_dns_cache, &s::arginfo_swoole_clear_dns_cache, 0;
        c"swoole_substr_unserialize" => f::zif_swoole_substr_unserialize, &s::arginfo_swoole_substr_unserialize, 0;
        c"swoole_substr_json_decode" => f::zif_swoole_substr_json_decode, &s::arginfo_swoole_substr_json_decode, 0;
        c"swoole_internal_call_user_shutdown_begin" => f::zif_swoole_internal_call_user_shutdown_begin, &s::arginfo_swoole_internal_call_user_shutdown_begin, 0;
        c"swoole_implicit_fn" => zif_swoole_implicit_fn, &s::arginfo_swoole_implicit_fn, 0;
        c"swoole_get_objects" => vm::zif_swoole_get_objects, &ex::arginfo_swoole_get_objects, 0;
        c"swoole_get_vm_status" => vm::zif_swoole_get_vm_status, &ex::arginfo_swoole_get_vm_status, 0;
        c"swoole_get_object_by_handle" => vm::zif_swoole_get_object_by_handle, &ex::arginfo_swoole_get_object_by_handle, 0;
        c"swoole_name_resolver_lookup" => zif_swoole_name_resolver_lookup, &ex::arginfo_swoole_name_resolver_lookup, 0;
        c"swoole_name_resolver_add" => zif_swoole_name_resolver_add, &ex::arginfo_swoole_name_resolver_add, 0;
        c"swoole_name_resolver_remove" => zif_swoole_name_resolver_remove, &ex::arginfo_swoole_name_resolver_remove, 0;
    ];
}

method_table! {
    static TAIL = [
        c"swoole_tracer_leak_detect" => zif_swoole_tracer_leak_detect, &tracer::arginfo_swoole_tracer_leak_detect, 0;
        c"swoole_tracer_prof_begin" => zif_swoole_tracer_prof_begin, &tracer::arginfo_swoole_tracer_prof_begin, 0;
        c"swoole_tracer_prof_end" => zif_swoole_tracer_prof_end, &tracer::arginfo_swoole_tracer_prof_end, 0;
    ];
}

/// `head`'s rows, then `optional`'s up to its `END`, then `tail` (which ends
/// with `END`), leaked for the process.
/// # Safety
/// `optional` is null or `END`-terminated.
unsafe fn join(
    head: &[zend_function_entry],
    mut optional: *const zend_function_entry,
    tail: &[zend_function_entry],
) -> *const zend_function_entry {
    let mut table = head.to_vec();
    // SAFETY: per the caller, every row up to END is readable.
    while let Some(row) = unsafe { optional.as_ref() }.filter(|row| !row.fname.is_null()) {
        table.push(*row);
        // SAFETY: not past END.
        optional = unsafe { optional.add(1) };
    }
    table.extend_from_slice(tail);
    Box::leak(table.into_boxed_slice()).as_ptr()
}

/// `swoole_functions[]`: Rust's rows around `optional` (the stdext, ssh2 and
/// ftp rows only C++'s `#ifdef`s decide). C++ calls it while initializing
/// `swoole_module_entry`, before Zend reads the entry.
/// # Safety
/// `optional` is null or a static `END`-terminated table.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpswoole_functions(
    optional: *const zend_function_entry,
) -> *const zend_function_entry {
    // SAFETY: per the caller; HEAD's last row is its END.
    unsafe { join(&HEAD.0[..HEAD.0.len() - 1], optional, &TAIL.0) }
}

/// MINIT's `go`, `defer` and (with stdext) `typed_array`, when short names are on.
/// # Safety
/// At MINIT, after Zend registered the module's functions.
pub(crate) unsafe fn register_aliases(flags: u32) {
    if flags & SHORTNAME == 0 {
        return;
    }
    let aliases: [(&CStr, &CStr, *const zend_internal_arg_info); 3] = [
        (
            c"swoole_coroutine_create",
            c"go",
            s::arginfo_swoole_coroutine_create.0.as_ptr(),
        ),
        (
            c"swoole_coroutine_defer",
            c"defer",
            s::arginfo_swoole_coroutine_defer.0.as_ptr(),
        ),
        (
            c"swoole_typed_array",
            c"typed_array",
            stdext::arginfo_swoole_typed_array.0.as_ptr(),
        ),
    ];
    let count = if flags & STDEXT == 0 { 2 } else { 3 };
    for (origin, alias, arg_info) in &aliases[..count] {
        // SAFETY: at MINIT; Zend copies both names and keeps the static arginfo.
        unsafe {
            z::swr_zend_register_global_alias(
                origin.as_ptr(),
                origin.count_bytes(),
                alias.as_ptr(),
                alias.count_bytes(),
                *arg_info,
            )
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swoole_rs_zend::arginfo::{ArgInfo, END, begin, entry};

    unsafe extern "C" fn handler(_: *mut z::zend_execute_data, _: *mut z::zval) {}
    static INFO: ArgInfo<1> = ArgInfo([begin(0, 0)]);

    fn names(mut table: *const zend_function_entry) -> Vec<&'static CStr> {
        let mut names = Vec::new();
        // SAFETY: an END-terminated table of static names.
        while let Some(row) = unsafe { table.as_ref() }.filter(|row| !row.fname.is_null()) {
            // SAFETY: as above.
            names.push(unsafe { CStr::from_ptr(row.fname) });
            // SAFETY: as above.
            table = unsafe { table.add(1) };
        }
        names
    }

    #[test]
    fn optional_rows_sit_between_head_and_tail() {
        let head = [
            entry(c"a", handler, &INFO, 0),
            entry(c"b", handler, &INFO, 0),
        ];
        let optional = [entry(c"c", handler, &INFO, 0), END];
        let tail = [entry(c"d", handler, &INFO, 0), END];
        // SAFETY: END-terminated.
        let table = unsafe { join(&head, optional.as_ptr(), &tail) };
        assert_eq!(names(table), [c"a", c"b", c"c", c"d"]);
    }

    #[test]
    fn no_optional_rows() {
        let head = [entry(c"a", handler, &INFO, 0)];
        let tail = [entry(c"d", handler, &INFO, 0), END];
        // SAFETY: null is allowed.
        let table = unsafe { join(&head, std::ptr::null(), &tail) };
        assert_eq!(names(table), [c"a", c"d"]);
    }
}
