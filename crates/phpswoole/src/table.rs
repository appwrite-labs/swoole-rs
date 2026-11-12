//! The module's function table (`swoole_functions[]` in `php_swoole.cc`), in
//! upstream's order, and the short-name aliases MINIT adds.
use crate::{
    asynchronous as a, constants::SHORTNAME, functions as f, implicit, interfaces as i, module, vm,
};
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
    fn zif_swoole_coroutine_create(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_swoole_coroutine_defer(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_swoole_coroutine_socketpair(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_swoole_test_kernel_coroutine(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_swoole_client_select(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_swoole_clear_dns_cache(frame: *mut z::zend_execute_data, value: *mut z::zval);
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
        c"swoole_implicit_fn" => implicit::zif_swoole_implicit_fn, &s::arginfo_swoole_implicit_fn, 0;
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

#[cfg(feature = "stdext")]
// crates/phpstdext's handlers.
unsafe extern "C" {
    fn zif_swoole_call_array_method(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_swoole_call_string_method(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_swoole_call_stream_method(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_swoole_array_search(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_swoole_array_contains(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_swoole_array_join(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_swoole_array_key_exists(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_swoole_array_map(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_swoole_str_split(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_swoole_parse_str(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_swoole_hash(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_swoole_typed_array(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_swoole_array_is_typed(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_swoole_str_is_empty(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_swoole_array_is_empty(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_swoole_str_match(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_swoole_str_match_all(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_swoole_str_json_decode(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_swoole_str_json_decode_to_object(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_swoole_str_replace(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_swoole_str_ireplace(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_swoole_array_replace_str(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_swoole_array_ireplace_str(frame: *mut z::zend_execute_data, value: *mut z::zval);
}

#[cfg(feature = "ssh2")]
// The bundled ext/ssh2's handlers (thirdparty/php/ssh2).
unsafe extern "C" {
    fn zif_ssh2_connect(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ssh2_disconnect(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ssh2_methods_negotiated(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ssh2_fingerprint(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ssh2_auth_none(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ssh2_auth_password(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ssh2_auth_pubkey_file(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ssh2_auth_pubkey(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ssh2_auth_hostbased_file(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ssh2_forward_listen(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ssh2_forward_accept(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ssh2_shell(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ssh2_shell_resize(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ssh2_exec(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ssh2_tunnel(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ssh2_scp_recv(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ssh2_scp_send(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ssh2_fetch_stream(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ssh2_send_eof(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ssh2_sftp(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ssh2_sftp_rename(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ssh2_sftp_unlink(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ssh2_sftp_mkdir(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ssh2_sftp_rmdir(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ssh2_sftp_chmod(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ssh2_sftp_stat(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ssh2_sftp_lstat(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ssh2_sftp_symlink(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ssh2_sftp_readlink(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ssh2_sftp_realpath(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ssh2_publickey_init(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ssh2_publickey_add(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ssh2_publickey_remove(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ssh2_publickey_list(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ssh2_auth_agent(frame: *mut z::zend_execute_data, value: *mut z::zval);
}

#[cfg(feature = "ftp")]
// The bundled ext/ftp's handlers (thirdparty/php84/ftp), with SSL.
unsafe extern "C" {
    fn zif_ftp_connect(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ftp_ssl_connect(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ftp_login(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ftp_pwd(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ftp_cdup(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ftp_chdir(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ftp_exec(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ftp_raw(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ftp_mkdir(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ftp_rmdir(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ftp_chmod(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ftp_alloc(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ftp_nlist(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ftp_rawlist(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ftp_mlsd(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ftp_systype(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ftp_fget(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ftp_nb_fget(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ftp_pasv(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ftp_get(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ftp_nb_get(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ftp_nb_continue(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ftp_fput(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ftp_nb_fput(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ftp_put(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ftp_append(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ftp_nb_put(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ftp_size(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ftp_mdtm(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ftp_rename(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ftp_delete(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ftp_site(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ftp_close(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ftp_set_option(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zif_ftp_get_option(frame: *mut z::zend_execute_data, value: *mut z::zval);
}

#[cfg(feature = "stdext")]
method_table! {
    static STDEXT_FUNCTIONS = [
        c"swoole_call_array_method" => zif_swoole_call_array_method, &stdext::arginfo_swoole_call_array_method, 0;
        c"swoole_call_string_method" => zif_swoole_call_string_method, &stdext::arginfo_swoole_call_string_method, 0;
        c"swoole_call_stream_method" => zif_swoole_call_stream_method, &stdext::arginfo_swoole_call_stream_method, 0;
        c"swoole_array_search" => zif_swoole_array_search, &stdext::arginfo_swoole_array_search, 0;
        c"swoole_array_contains" => zif_swoole_array_contains, &stdext::arginfo_swoole_array_contains, 0;
        c"swoole_array_join" => zif_swoole_array_join, &stdext::arginfo_swoole_array_join, 0;
        c"swoole_array_key_exists" => zif_swoole_array_key_exists, &stdext::arginfo_swoole_array_key_exists, 0;
        c"swoole_array_map" => zif_swoole_array_map, &stdext::arginfo_swoole_array_map, 0;
        c"swoole_str_split" => zif_swoole_str_split, &stdext::arginfo_swoole_str_split, 0;
        c"swoole_parse_str" => zif_swoole_parse_str, &stdext::arginfo_swoole_parse_str, 0;
        c"swoole_hash" => zif_swoole_hash, &stdext::arginfo_swoole_hash, 0;
        c"swoole_typed_array" => zif_swoole_typed_array, &stdext::arginfo_swoole_typed_array, 0;
        c"swoole_array_is_typed" => zif_swoole_array_is_typed, &stdext::arginfo_swoole_array_is_typed, 0;
        c"swoole_str_is_empty" => zif_swoole_str_is_empty, &stdext::arginfo_swoole_str_is_empty, 0;
        c"swoole_array_is_empty" => zif_swoole_array_is_empty, &stdext::arginfo_swoole_array_is_empty, 0;
        c"swoole_str_match" => zif_swoole_str_match, &stdext::arginfo_swoole_str_match, 0;
        c"swoole_str_match_all" => zif_swoole_str_match_all, &stdext::arginfo_swoole_str_match_all, 0;
        c"swoole_str_json_decode" => zif_swoole_str_json_decode, &stdext::arginfo_swoole_str_json_decode, 0;
        c"swoole_str_json_decode_to_object" => zif_swoole_str_json_decode_to_object, &stdext::arginfo_swoole_str_json_decode_to_object, 0;
        c"swoole_str_replace" => zif_swoole_str_replace, &stdext::arginfo_swoole_str_replace, 0;
        c"swoole_str_ireplace" => zif_swoole_str_ireplace, &stdext::arginfo_swoole_str_ireplace, 0;
        c"swoole_array_replace_str" => zif_swoole_array_replace_str, &stdext::arginfo_swoole_array_replace_str, 0;
        c"swoole_array_ireplace_str" => zif_swoole_array_ireplace_str, &stdext::arginfo_swoole_array_ireplace_str, 0;
    ];
}

#[cfg(feature = "ssh2")]
method_table! {
    static SSH2_FUNCTIONS = [
        c"ssh2_connect" => zif_ssh2_connect, &swoole_rs_phpstubs::ssh2::arginfo_ssh2_connect, 0;
        c"ssh2_disconnect" => zif_ssh2_disconnect, &swoole_rs_phpstubs::ssh2::arginfo_ssh2_disconnect, 0;
        c"ssh2_methods_negotiated" => zif_ssh2_methods_negotiated, &swoole_rs_phpstubs::ssh2::arginfo_ssh2_methods_negotiated, 0;
        c"ssh2_fingerprint" => zif_ssh2_fingerprint, &swoole_rs_phpstubs::ssh2::arginfo_ssh2_fingerprint, 0;
        c"ssh2_auth_none" => zif_ssh2_auth_none, &swoole_rs_phpstubs::ssh2::arginfo_ssh2_auth_none, 0;
        c"ssh2_auth_password" => zif_ssh2_auth_password, &swoole_rs_phpstubs::ssh2::arginfo_ssh2_auth_password, 0;
        c"ssh2_auth_pubkey_file" => zif_ssh2_auth_pubkey_file, &swoole_rs_phpstubs::ssh2::arginfo_ssh2_auth_pubkey_file, 0;
        c"ssh2_auth_pubkey" => zif_ssh2_auth_pubkey, &swoole_rs_phpstubs::ssh2::arginfo_ssh2_auth_pubkey, 0;
        c"ssh2_auth_hostbased_file" => zif_ssh2_auth_hostbased_file, &swoole_rs_phpstubs::ssh2::arginfo_ssh2_auth_hostbased_file, 0;
        c"ssh2_forward_listen" => zif_ssh2_forward_listen, &swoole_rs_phpstubs::ssh2::arginfo_ssh2_forward_listen, 0;
        c"ssh2_forward_accept" => zif_ssh2_forward_accept, &swoole_rs_phpstubs::ssh2::arginfo_ssh2_forward_accept, 0;
        c"ssh2_shell" => zif_ssh2_shell, &swoole_rs_phpstubs::ssh2::arginfo_ssh2_shell, 0;
        c"ssh2_shell_resize" => zif_ssh2_shell_resize, &swoole_rs_phpstubs::ssh2::arginfo_ssh2_shell_resize, 0;
        c"ssh2_exec" => zif_ssh2_exec, &swoole_rs_phpstubs::ssh2::arginfo_ssh2_exec, 0;
        c"ssh2_tunnel" => zif_ssh2_tunnel, &swoole_rs_phpstubs::ssh2::arginfo_ssh2_tunnel, 0;
        c"ssh2_scp_recv" => zif_ssh2_scp_recv, &swoole_rs_phpstubs::ssh2::arginfo_ssh2_scp_recv, 0;
        c"ssh2_scp_send" => zif_ssh2_scp_send, &swoole_rs_phpstubs::ssh2::arginfo_ssh2_scp_send, 0;
        c"ssh2_fetch_stream" => zif_ssh2_fetch_stream, &swoole_rs_phpstubs::ssh2::arginfo_ssh2_fetch_stream, 0;
        c"ssh2_send_eof" => zif_ssh2_send_eof, &swoole_rs_phpstubs::ssh2::arginfo_ssh2_send_eof, 0;
        c"ssh2_sftp" => zif_ssh2_sftp, &swoole_rs_phpstubs::ssh2::arginfo_ssh2_sftp, 0;
        c"ssh2_sftp_rename" => zif_ssh2_sftp_rename, &swoole_rs_phpstubs::ssh2::arginfo_ssh2_sftp_rename, 0;
        c"ssh2_sftp_unlink" => zif_ssh2_sftp_unlink, &swoole_rs_phpstubs::ssh2::arginfo_ssh2_sftp_unlink, 0;
        c"ssh2_sftp_mkdir" => zif_ssh2_sftp_mkdir, &swoole_rs_phpstubs::ssh2::arginfo_ssh2_sftp_mkdir, 0;
        c"ssh2_sftp_rmdir" => zif_ssh2_sftp_rmdir, &swoole_rs_phpstubs::ssh2::arginfo_ssh2_sftp_rmdir, 0;
        c"ssh2_sftp_chmod" => zif_ssh2_sftp_chmod, &swoole_rs_phpstubs::ssh2::arginfo_ssh2_sftp_chmod, 0;
        c"ssh2_sftp_stat" => zif_ssh2_sftp_stat, &swoole_rs_phpstubs::ssh2::arginfo_ssh2_sftp_stat, 0;
        c"ssh2_sftp_lstat" => zif_ssh2_sftp_lstat, &swoole_rs_phpstubs::ssh2::arginfo_ssh2_sftp_lstat, 0;
        c"ssh2_sftp_symlink" => zif_ssh2_sftp_symlink, &swoole_rs_phpstubs::ssh2::arginfo_ssh2_sftp_symlink, 0;
        c"ssh2_sftp_readlink" => zif_ssh2_sftp_readlink, &swoole_rs_phpstubs::ssh2::arginfo_ssh2_sftp_readlink, 0;
        c"ssh2_sftp_realpath" => zif_ssh2_sftp_realpath, &swoole_rs_phpstubs::ssh2::arginfo_ssh2_sftp_realpath, 0;
        c"ssh2_publickey_init" => zif_ssh2_publickey_init, &swoole_rs_phpstubs::ssh2::arginfo_ssh2_publickey_init, 0;
        c"ssh2_publickey_add" => zif_ssh2_publickey_add, &swoole_rs_phpstubs::ssh2::arginfo_ssh2_publickey_add, 0;
        c"ssh2_publickey_remove" => zif_ssh2_publickey_remove, &swoole_rs_phpstubs::ssh2::arginfo_ssh2_publickey_remove, 0;
        c"ssh2_publickey_list" => zif_ssh2_publickey_list, &swoole_rs_phpstubs::ssh2::arginfo_ssh2_publickey_list, 0;
        c"ssh2_auth_agent" => zif_ssh2_auth_agent, &swoole_rs_phpstubs::ssh2::arginfo_ssh2_auth_agent, 0;
    ];
}

#[cfg(feature = "ftp")]
method_table! {
    static FTP_FUNCTIONS = [
        c"ftp_connect" => zif_ftp_connect, &swoole_rs_phpstubs::ftp::arginfo_ftp_connect, 0;
        c"ftp_ssl_connect" => zif_ftp_ssl_connect, &swoole_rs_phpstubs::ftp::arginfo_ftp_ssl_connect, 0;
        c"ftp_login" => zif_ftp_login, &swoole_rs_phpstubs::ftp::arginfo_ftp_login, 0;
        c"ftp_pwd" => zif_ftp_pwd, &swoole_rs_phpstubs::ftp::arginfo_ftp_pwd, 0;
        c"ftp_cdup" => zif_ftp_cdup, &swoole_rs_phpstubs::ftp::arginfo_ftp_cdup, 0;
        c"ftp_chdir" => zif_ftp_chdir, &swoole_rs_phpstubs::ftp::arginfo_ftp_chdir, 0;
        c"ftp_exec" => zif_ftp_exec, &swoole_rs_phpstubs::ftp::arginfo_ftp_exec, 0;
        c"ftp_raw" => zif_ftp_raw, &swoole_rs_phpstubs::ftp::arginfo_ftp_raw, 0;
        c"ftp_mkdir" => zif_ftp_mkdir, &swoole_rs_phpstubs::ftp::arginfo_ftp_mkdir, 0;
        c"ftp_rmdir" => zif_ftp_rmdir, &swoole_rs_phpstubs::ftp::arginfo_ftp_rmdir, 0;
        c"ftp_chmod" => zif_ftp_chmod, &swoole_rs_phpstubs::ftp::arginfo_ftp_chmod, 0;
        c"ftp_alloc" => zif_ftp_alloc, &swoole_rs_phpstubs::ftp::arginfo_ftp_alloc, 0;
        c"ftp_nlist" => zif_ftp_nlist, &swoole_rs_phpstubs::ftp::arginfo_ftp_nlist, 0;
        c"ftp_rawlist" => zif_ftp_rawlist, &swoole_rs_phpstubs::ftp::arginfo_ftp_rawlist, 0;
        c"ftp_mlsd" => zif_ftp_mlsd, &swoole_rs_phpstubs::ftp::arginfo_ftp_mlsd, 0;
        c"ftp_systype" => zif_ftp_systype, &swoole_rs_phpstubs::ftp::arginfo_ftp_systype, 0;
        c"ftp_fget" => zif_ftp_fget, &swoole_rs_phpstubs::ftp::arginfo_ftp_fget, 0;
        c"ftp_nb_fget" => zif_ftp_nb_fget, &swoole_rs_phpstubs::ftp::arginfo_ftp_nb_fget, 0;
        c"ftp_pasv" => zif_ftp_pasv, &swoole_rs_phpstubs::ftp::arginfo_ftp_pasv, 0;
        c"ftp_get" => zif_ftp_get, &swoole_rs_phpstubs::ftp::arginfo_ftp_get, 0;
        c"ftp_nb_get" => zif_ftp_nb_get, &swoole_rs_phpstubs::ftp::arginfo_ftp_nb_get, 0;
        c"ftp_nb_continue" => zif_ftp_nb_continue, &swoole_rs_phpstubs::ftp::arginfo_ftp_nb_continue, 0;
        c"ftp_fput" => zif_ftp_fput, &swoole_rs_phpstubs::ftp::arginfo_ftp_fput, 0;
        c"ftp_nb_fput" => zif_ftp_nb_fput, &swoole_rs_phpstubs::ftp::arginfo_ftp_nb_fput, 0;
        c"ftp_put" => zif_ftp_put, &swoole_rs_phpstubs::ftp::arginfo_ftp_put, 0;
        c"ftp_append" => zif_ftp_append, &swoole_rs_phpstubs::ftp::arginfo_ftp_append, 0;
        c"ftp_nb_put" => zif_ftp_nb_put, &swoole_rs_phpstubs::ftp::arginfo_ftp_nb_put, 0;
        c"ftp_size" => zif_ftp_size, &swoole_rs_phpstubs::ftp::arginfo_ftp_size, 0;
        c"ftp_mdtm" => zif_ftp_mdtm, &swoole_rs_phpstubs::ftp::arginfo_ftp_mdtm, 0;
        c"ftp_rename" => zif_ftp_rename, &swoole_rs_phpstubs::ftp::arginfo_ftp_rename, 0;
        c"ftp_delete" => zif_ftp_delete, &swoole_rs_phpstubs::ftp::arginfo_ftp_delete, 0;
        c"ftp_site" => zif_ftp_site, &swoole_rs_phpstubs::ftp::arginfo_ftp_site, 0;
        c"ftp_close" => zif_ftp_close, &swoole_rs_phpstubs::ftp::arginfo_ftp_close, 0;
        c"ftp_quit" => zif_ftp_close, &swoole_rs_phpstubs::ftp::arginfo_ftp_quit, 0;
        c"ftp_set_option" => zif_ftp_set_option, &swoole_rs_phpstubs::ftp::arginfo_ftp_set_option, 0;
        c"ftp_get_option" => zif_ftp_get_option, &swoole_rs_phpstubs::ftp::arginfo_ftp_get_option, 0;
    ];
}

/// The complete table, in upstream's order: the optional families the build
/// has sit between the head and the tracer's functions.
pub(crate) fn functions() -> *const zend_function_entry {
    let rows = |table: &'static [zend_function_entry]| {
        table.split_last().map_or(&[][..], |(_, rows)| rows)
    };
    let mut table = rows(&HEAD.0).to_vec();
    #[cfg(feature = "stdext")]
    table.extend_from_slice(rows(&STDEXT_FUNCTIONS.0));
    #[cfg(feature = "ssh2")]
    table.extend_from_slice(rows(&SSH2_FUNCTIONS.0));
    #[cfg(feature = "ftp")]
    table.extend_from_slice(rows(&FTP_FUNCTIONS.0));
    table.extend_from_slice(&TAIL.0);
    Box::leak(table.into_boxed_slice()).as_ptr()
}

/// MINIT's `go`, `defer` and (with stdext) `typed_array`, when short names are on.
/// The token is the module's MINIT, which Zend runs after registering the
/// module's functions.
pub(crate) fn register_aliases(
    minit: &swoole_rs_zend::Minit,
    flags: u32,
) -> Result<(), swoole_rs_zend::bailout::Bailout> {
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
    for (origin, alias, arg_info) in aliases.iter().take(count) {
        let target = minit.compiler_functions();
        // SAFETY: at MINIT, after Zend registered the module's internal
        // functions in the compiler's table; these static names and arginfo
        // remain live while the registry performs the contained registration.
        unsafe {
            swoole_rs_zend::registration::function_alias(target, origin, target, alias, *arg_info)
        }?;
    }
    Ok(())
}
