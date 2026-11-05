//! The module's function table (`swoole_functions[]` in `php_swoole.cc`), in
//! upstream's order, and the short-name aliases MINIT adds.
use crate::{asynchronous as a, constants::SHORTNAME, functions as f, interfaces as i, module, vm};
use std::ffi::CStr;
use swoole_rs_phpstubs::{ex, ftp, ssh2, stdext, swoole as s, tracer};
use swoole_rs_zend::{
    arginfo::{Handler, Methods, definition},
    method_table,
    sys::{self as z, zend_function_entry, zend_internal_arg_info},
};

/// `swr_v1_phpswoole_minit` flag: the build has `SW_STDEXT`.
pub(crate) const STDEXT: u32 = 1 << 11;

// Handlers other crates or C++ define, whichever the build selects.
unsafe extern "C" {
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
        c"swoole_async_dns_lookup_coro" => a::zif_swoole_async_dns_lookup_coro, &s::arginfo_swoole_async_dns_lookup_coro, 0;
        c"swoole_async_set" => a::zif_swoole_async_set, &s::arginfo_swoole_async_set, 0;
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

// Optional native handlers; generated Rust arginfo remains authoritative.
static STDEXT_ROWS: Methods<23> = Methods([
    definition(
        c"swoole_call_array_method",
        &stdext::arginfo_swoole_call_array_method,
        0,
    ),
    definition(
        c"swoole_call_string_method",
        &stdext::arginfo_swoole_call_string_method,
        0,
    ),
    definition(
        c"swoole_call_stream_method",
        &stdext::arginfo_swoole_call_stream_method,
        0,
    ),
    definition(
        c"swoole_array_search",
        &stdext::arginfo_swoole_array_search,
        0,
    ),
    definition(
        c"swoole_array_contains",
        &stdext::arginfo_swoole_array_contains,
        0,
    ),
    definition(c"swoole_array_join", &stdext::arginfo_swoole_array_join, 0),
    definition(
        c"swoole_array_key_exists",
        &stdext::arginfo_swoole_array_key_exists,
        0,
    ),
    definition(c"swoole_array_map", &stdext::arginfo_swoole_array_map, 0),
    definition(c"swoole_str_split", &stdext::arginfo_swoole_str_split, 0),
    definition(c"swoole_parse_str", &stdext::arginfo_swoole_parse_str, 0),
    definition(c"swoole_hash", &stdext::arginfo_swoole_hash, 0),
    definition(
        c"swoole_typed_array",
        &stdext::arginfo_swoole_typed_array,
        0,
    ),
    definition(
        c"swoole_array_is_typed",
        &stdext::arginfo_swoole_array_is_typed,
        0,
    ),
    definition(
        c"swoole_str_is_empty",
        &stdext::arginfo_swoole_str_is_empty,
        0,
    ),
    definition(
        c"swoole_array_is_empty",
        &stdext::arginfo_swoole_array_is_empty,
        0,
    ),
    definition(c"swoole_str_match", &stdext::arginfo_swoole_str_match, 0),
    definition(
        c"swoole_str_match_all",
        &stdext::arginfo_swoole_str_match_all,
        0,
    ),
    definition(
        c"swoole_str_json_decode",
        &stdext::arginfo_swoole_str_json_decode,
        0,
    ),
    definition(
        c"swoole_str_json_decode_to_object",
        &stdext::arginfo_swoole_str_json_decode_to_object,
        0,
    ),
    definition(
        c"swoole_str_replace",
        &stdext::arginfo_swoole_str_replace,
        0,
    ),
    definition(
        c"swoole_str_ireplace",
        &stdext::arginfo_swoole_str_ireplace,
        0,
    ),
    definition(
        c"swoole_array_replace_str",
        &stdext::arginfo_swoole_array_replace_str,
        0,
    ),
    definition(
        c"swoole_array_ireplace_str",
        &stdext::arginfo_swoole_array_ireplace_str,
        0,
    ),
]);
static SSH2_ROWS: Methods<35> = Methods([
    definition(c"ssh2_connect", &ssh2::arginfo_ssh2_connect, 0),
    definition(c"ssh2_disconnect", &ssh2::arginfo_ssh2_disconnect, 0),
    definition(
        c"ssh2_methods_negotiated",
        &ssh2::arginfo_ssh2_methods_negotiated,
        0,
    ),
    definition(c"ssh2_fingerprint", &ssh2::arginfo_ssh2_fingerprint, 0),
    definition(c"ssh2_auth_none", &ssh2::arginfo_ssh2_auth_none, 0),
    definition(c"ssh2_auth_password", &ssh2::arginfo_ssh2_auth_password, 0),
    definition(
        c"ssh2_auth_pubkey_file",
        &ssh2::arginfo_ssh2_auth_pubkey_file,
        0,
    ),
    definition(c"ssh2_auth_pubkey", &ssh2::arginfo_ssh2_auth_pubkey, 0),
    definition(
        c"ssh2_auth_hostbased_file",
        &ssh2::arginfo_ssh2_auth_hostbased_file,
        0,
    ),
    definition(
        c"ssh2_forward_listen",
        &ssh2::arginfo_ssh2_forward_listen,
        0,
    ),
    definition(
        c"ssh2_forward_accept",
        &ssh2::arginfo_ssh2_forward_accept,
        0,
    ),
    definition(c"ssh2_shell", &ssh2::arginfo_ssh2_shell, 0),
    definition(c"ssh2_shell_resize", &ssh2::arginfo_ssh2_shell_resize, 0),
    definition(c"ssh2_exec", &ssh2::arginfo_ssh2_exec, 0),
    definition(c"ssh2_tunnel", &ssh2::arginfo_ssh2_tunnel, 0),
    definition(c"ssh2_scp_recv", &ssh2::arginfo_ssh2_scp_recv, 0),
    definition(c"ssh2_scp_send", &ssh2::arginfo_ssh2_scp_send, 0),
    definition(c"ssh2_fetch_stream", &ssh2::arginfo_ssh2_fetch_stream, 0),
    definition(c"ssh2_send_eof", &ssh2::arginfo_ssh2_send_eof, 0),
    definition(c"ssh2_sftp", &ssh2::arginfo_ssh2_sftp, 0),
    definition(c"ssh2_sftp_rename", &ssh2::arginfo_ssh2_sftp_rename, 0),
    definition(c"ssh2_sftp_unlink", &ssh2::arginfo_ssh2_sftp_unlink, 0),
    definition(c"ssh2_sftp_mkdir", &ssh2::arginfo_ssh2_sftp_mkdir, 0),
    definition(c"ssh2_sftp_rmdir", &ssh2::arginfo_ssh2_sftp_rmdir, 0),
    definition(c"ssh2_sftp_chmod", &ssh2::arginfo_ssh2_sftp_chmod, 0),
    definition(c"ssh2_sftp_stat", &ssh2::arginfo_ssh2_sftp_stat, 0),
    definition(c"ssh2_sftp_lstat", &ssh2::arginfo_ssh2_sftp_lstat, 0),
    definition(c"ssh2_sftp_symlink", &ssh2::arginfo_ssh2_sftp_symlink, 0),
    definition(c"ssh2_sftp_readlink", &ssh2::arginfo_ssh2_sftp_readlink, 0),
    definition(c"ssh2_sftp_realpath", &ssh2::arginfo_ssh2_sftp_realpath, 0),
    definition(
        c"ssh2_publickey_init",
        &ssh2::arginfo_ssh2_publickey_init,
        0,
    ),
    definition(c"ssh2_publickey_add", &ssh2::arginfo_ssh2_publickey_add, 0),
    definition(
        c"ssh2_publickey_remove",
        &ssh2::arginfo_ssh2_publickey_remove,
        0,
    ),
    definition(
        c"ssh2_publickey_list",
        &ssh2::arginfo_ssh2_publickey_list,
        0,
    ),
    definition(c"ssh2_auth_agent", &ssh2::arginfo_ssh2_auth_agent, 0),
]);
static FTP_ROWS: Methods<36> = Methods([
    definition(c"ftp_connect", &ftp::arginfo_ftp_connect, 0),
    definition(c"ftp_ssl_connect", &ftp::arginfo_ftp_ssl_connect, 0),
    definition(c"ftp_login", &ftp::arginfo_ftp_login, 0),
    definition(c"ftp_pwd", &ftp::arginfo_ftp_pwd, 0),
    definition(c"ftp_cdup", &ftp::arginfo_ftp_cdup, 0),
    definition(c"ftp_chdir", &ftp::arginfo_ftp_chdir, 0),
    definition(c"ftp_exec", &ftp::arginfo_ftp_exec, 0),
    definition(c"ftp_raw", &ftp::arginfo_ftp_raw, 0),
    definition(c"ftp_mkdir", &ftp::arginfo_ftp_mkdir, 0),
    definition(c"ftp_rmdir", &ftp::arginfo_ftp_rmdir, 0),
    definition(c"ftp_chmod", &ftp::arginfo_ftp_chmod, 0),
    definition(c"ftp_alloc", &ftp::arginfo_ftp_alloc, 0),
    definition(c"ftp_nlist", &ftp::arginfo_ftp_nlist, 0),
    definition(c"ftp_rawlist", &ftp::arginfo_ftp_rawlist, 0),
    definition(c"ftp_mlsd", &ftp::arginfo_ftp_mlsd, 0),
    definition(c"ftp_systype", &ftp::arginfo_ftp_systype, 0),
    definition(c"ftp_fget", &ftp::arginfo_ftp_fget, 0),
    definition(c"ftp_nb_fget", &ftp::arginfo_ftp_nb_fget, 0),
    definition(c"ftp_pasv", &ftp::arginfo_ftp_pasv, 0),
    definition(c"ftp_get", &ftp::arginfo_ftp_get, 0),
    definition(c"ftp_nb_get", &ftp::arginfo_ftp_nb_get, 0),
    definition(c"ftp_nb_continue", &ftp::arginfo_ftp_nb_continue, 0),
    definition(c"ftp_fput", &ftp::arginfo_ftp_fput, 0),
    definition(c"ftp_nb_fput", &ftp::arginfo_ftp_nb_fput, 0),
    definition(c"ftp_put", &ftp::arginfo_ftp_put, 0),
    definition(c"ftp_append", &ftp::arginfo_ftp_append, 0),
    definition(c"ftp_nb_put", &ftp::arginfo_ftp_nb_put, 0),
    definition(c"ftp_size", &ftp::arginfo_ftp_size, 0),
    definition(c"ftp_mdtm", &ftp::arginfo_ftp_mdtm, 0),
    definition(c"ftp_rename", &ftp::arginfo_ftp_rename, 0),
    definition(c"ftp_delete", &ftp::arginfo_ftp_delete, 0),
    definition(c"ftp_site", &ftp::arginfo_ftp_site, 0),
    definition(c"ftp_close", &ftp::arginfo_ftp_close, 0),
    definition(c"ftp_quit", &ftp::arginfo_ftp_quit, 0),
    definition(c"ftp_set_option", &ftp::arginfo_ftp_set_option, 0),
    definition(c"ftp_get_option", &ftp::arginfo_ftp_get_option, 0),
]);

/// Append a family's Rust metadata with its static native handler array.
/// # Safety
/// `handlers` is null (family disabled), or has one entry for every metadata row.
/// Only an unavailable FTP SSL handler may be null within an enabled family.
unsafe fn append_optional(
    table: &mut Vec<zend_function_entry>,
    rows: &[zend_function_entry],
    handlers: *const Option<Handler>,
) {
    if handlers.is_null() {
        return;
    }
    for (index, row) in rows.iter().enumerate() {
        // SAFETY: the caller lends an entry for every row.
        let handler = unsafe { *handlers.wrapping_add(index) };
        if handler.is_none() {
            // SAFETY: metadata names are static C strings.
            let name = unsafe { CStr::from_ptr(row.fname) };
            assert_eq!(name, c"ftp_ssl_connect", "missing native function handler");
            continue;
        }
        let mut entry = *row;
        entry.handler = handler;
        table.push(entry);
    }
}

/// Assemble the complete table in upstream family order.
/// # Safety
/// Each non-null native array has one element for every metadata row.
unsafe fn join(
    head: &[zend_function_entry],
    families: &[(&[zend_function_entry], *const Option<Handler>)],
    tail: &[zend_function_entry],
) -> *const zend_function_entry {
    let mut table = head.to_vec();
    for (rows, handlers) in families {
        // SAFETY: the caller lends an array matching these metadata rows.
        unsafe { append_optional(&mut table, rows, *handlers) };
    }
    table.extend_from_slice(tail);
    Box::leak(table.into_boxed_slice()).as_ptr()
}

/// Rust owns the complete module function table, including optional families.
/// Native lends only their handler addresses, in upstream's order.
/// # Safety
/// Each non-null array is static, with 23 stdext, 35 SSH2 or 36 FTP entries.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpswoole_functions(
    stdext: *const Option<Handler>,
    ssh2: *const Option<Handler>,
    ftp: *const Option<Handler>,
) -> *const zend_function_entry {
    let families = [
        (STDEXT_ROWS.0.as_slice(), stdext),
        (SSH2_ROWS.0.as_slice(), ssh2),
        (FTP_ROWS.0.as_slice(), ftp),
    ];
    // SAFETY: Native lends the matching arrays or null for disabled families.
    unsafe { join(&HEAD.0[..HEAD.0.len() - 1], &families, &TAIL.0) }
}

/// MINIT's `go`, `defer` and (with stdext) `typed_array`, when short names are on.
/// # Safety
/// At MINIT, after Zend registered the module's functions.
pub(crate) unsafe fn register_aliases(flags: u32) -> Result<(), swoole_rs_zend::bailout::Bailout> {
    if flags & SHORTNAME == 0 {
        return Ok(());
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
        // SAFETY: Zend initializes the compiler function table before MINIT.
        let target = unsafe { z::swr_zend_facade_compiler_functions() };
        // SAFETY: at MINIT, these static binary names and arginfo remain live
        // while the Rust registry performs the complete contained registration.
        unsafe {
            swoole_rs_zend::registration::function_alias(
                target,
                origin.as_ptr(),
                origin.count_bytes(),
                target,
                alias.as_ptr(),
                alias.count_bytes(),
                *arg_info,
                None,
            )
        }?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use swoole_rs_zend::arginfo::{ArgInfo, END, begin};

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
        let head = [definition(c"a", &INFO, 0), definition(c"b", &INFO, 0)];
        let optional = [definition(c"c", &INFO, 0)];
        let handlers = [Some(handler as Handler)];
        let tail = [definition(c"d", &INFO, 0), END];
        let table = unsafe { join(&head, &[(&optional, handlers.as_ptr())], &tail) };
        assert_eq!(names(table), [c"a", c"b", c"c", c"d"]);
    }

    #[test]
    fn no_optional_rows() {
        let head = [definition(c"a", &INFO, 0)];
        let tail = [definition(c"d", &INFO, 0), END];
        let table = unsafe { join(&head, &[], &tail) };
        assert_eq!(names(table), [c"a", c"d"]);
    }

    #[test]
    fn optional_registry_profiles_preserve_order_and_signatures() {
        let stdext = [Some(handler as Handler); 23];
        let ssh2 = [Some(handler as Handler); 35];
        for enabled in 0_u8..16 {
            let mut ftp = [Some(handler as Handler); 36];
            if enabled & 8 == 0 {
                ftp[1] = None;
            }
            let families = [
                (
                    STDEXT_ROWS.0.as_slice(),
                    if enabled & 1 == 0 {
                        std::ptr::null()
                    } else {
                        stdext.as_ptr()
                    },
                ),
                (
                    SSH2_ROWS.0.as_slice(),
                    if enabled & 2 == 0 {
                        std::ptr::null()
                    } else {
                        ssh2.as_ptr()
                    },
                ),
                (
                    FTP_ROWS.0.as_slice(),
                    if enabled & 4 == 0 {
                        std::ptr::null()
                    } else {
                        ftp.as_ptr()
                    },
                ),
            ];
            let table = unsafe { join(&[], &families, &[END]) };
            let mut index = 0;
            for (metadata, native) in families {
                if native.is_null() {
                    continue;
                }
                for (slot, info) in metadata.iter().enumerate() {
                    let handler = unsafe { *native.add(slot) };
                    if handler.is_none() {
                        continue;
                    }
                    let actual = unsafe { &*table.add(index) };
                    assert_eq!(actual.fname, info.fname);
                    assert_eq!(actual.arg_info, info.arg_info);
                    assert_eq!(actual.num_args, info.num_args);
                    assert_eq!(actual.flags, info.flags);
                    assert!(actual.handler.is_some());
                    index += 1;
                }
            }
            assert!(unsafe { (*table.add(index)).fname }.is_null());
            let names = names(table);
            assert_eq!(names.contains(&c"ftp_ssl_connect"), enabled & 12 == 12);
            if enabled & 4 != 0 {
                assert!(names.contains(&c"ftp_quit"));
                assert!(names.contains(&c"ftp_close"));
            }
        }
    }
}
