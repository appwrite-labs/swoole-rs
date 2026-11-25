//! The module's function table (`swoole_functions[]` in `php_swoole.cc`), in
//! upstream's order, and the short-name aliases MINIT adds.
use crate::{
    asynchronous as a, constants::SHORTNAME, functions as f, implicit, interfaces as i, module, vm,
};
use swoole_rs_phpstubs::{ex, swoole as s, tracer};
use swoole_rs_zend::{Entered, Returned, arginfo::Table, method_table, sys::zend_function_entry};

// Handlers other crates or C++ define, whichever the build selects: each takes
// the frame and return slot only Zend makes, as a `handlers!` one does.
unsafe extern "C" {
    safe fn zif_swoole_coroutine_create(frame: Entered<'_>, value: Returned<'_>);
    safe fn zif_swoole_coroutine_defer(frame: Entered<'_>, value: Returned<'_>);
    safe fn zif_swoole_coroutine_socketpair(frame: Entered<'_>, value: Returned<'_>);
    safe fn zif_swoole_test_kernel_coroutine(frame: Entered<'_>, value: Returned<'_>);
    safe fn zif_swoole_client_select(frame: Entered<'_>, value: Returned<'_>);
    safe fn zif_swoole_clear_dns_cache(frame: Entered<'_>, value: Returned<'_>);
    safe fn zif_swoole_name_resolver_lookup(frame: Entered<'_>, value: Returned<'_>);
    safe fn zif_swoole_name_resolver_add(frame: Entered<'_>, value: Returned<'_>);
    safe fn zif_swoole_name_resolver_remove(frame: Entered<'_>, value: Returned<'_>);
    safe fn zif_swoole_tracer_leak_detect(frame: Entered<'_>, value: Returned<'_>);
    safe fn zif_swoole_tracer_prof_begin(frame: Entered<'_>, value: Returned<'_>);
    safe fn zif_swoole_tracer_prof_end(frame: Entered<'_>, value: Returned<'_>);
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

#[cfg(feature = "ssh2")]
// The bundled ext/ssh2's handlers (crates/ssh2).
unsafe extern "C" {
    safe fn zif_ssh2_connect(frame: Entered<'_>, value: Returned<'_>);
    safe fn zif_ssh2_disconnect(frame: Entered<'_>, value: Returned<'_>);
    safe fn zif_ssh2_methods_negotiated(frame: Entered<'_>, value: Returned<'_>);
    safe fn zif_ssh2_fingerprint(frame: Entered<'_>, value: Returned<'_>);
    safe fn zif_ssh2_auth_none(frame: Entered<'_>, value: Returned<'_>);
    safe fn zif_ssh2_auth_password(frame: Entered<'_>, value: Returned<'_>);
    safe fn zif_ssh2_auth_pubkey_file(frame: Entered<'_>, value: Returned<'_>);
    safe fn zif_ssh2_auth_pubkey(frame: Entered<'_>, value: Returned<'_>);
    safe fn zif_ssh2_auth_hostbased_file(frame: Entered<'_>, value: Returned<'_>);
    safe fn zif_ssh2_forward_listen(frame: Entered<'_>, value: Returned<'_>);
    safe fn zif_ssh2_forward_accept(frame: Entered<'_>, value: Returned<'_>);
    safe fn zif_ssh2_shell(frame: Entered<'_>, value: Returned<'_>);
    safe fn zif_ssh2_shell_resize(frame: Entered<'_>, value: Returned<'_>);
    safe fn zif_ssh2_exec(frame: Entered<'_>, value: Returned<'_>);
    safe fn zif_ssh2_tunnel(frame: Entered<'_>, value: Returned<'_>);
    safe fn zif_ssh2_scp_recv(frame: Entered<'_>, value: Returned<'_>);
    safe fn zif_ssh2_scp_send(frame: Entered<'_>, value: Returned<'_>);
    safe fn zif_ssh2_fetch_stream(frame: Entered<'_>, value: Returned<'_>);
    safe fn zif_ssh2_send_eof(frame: Entered<'_>, value: Returned<'_>);
    safe fn zif_ssh2_sftp(frame: Entered<'_>, value: Returned<'_>);
    safe fn zif_ssh2_sftp_rename(frame: Entered<'_>, value: Returned<'_>);
    safe fn zif_ssh2_sftp_unlink(frame: Entered<'_>, value: Returned<'_>);
    safe fn zif_ssh2_sftp_mkdir(frame: Entered<'_>, value: Returned<'_>);
    safe fn zif_ssh2_sftp_rmdir(frame: Entered<'_>, value: Returned<'_>);
    safe fn zif_ssh2_sftp_chmod(frame: Entered<'_>, value: Returned<'_>);
    safe fn zif_ssh2_sftp_stat(frame: Entered<'_>, value: Returned<'_>);
    safe fn zif_ssh2_sftp_lstat(frame: Entered<'_>, value: Returned<'_>);
    safe fn zif_ssh2_sftp_symlink(frame: Entered<'_>, value: Returned<'_>);
    safe fn zif_ssh2_sftp_readlink(frame: Entered<'_>, value: Returned<'_>);
    safe fn zif_ssh2_sftp_realpath(frame: Entered<'_>, value: Returned<'_>);
    safe fn zif_ssh2_publickey_init(frame: Entered<'_>, value: Returned<'_>);
    safe fn zif_ssh2_publickey_add(frame: Entered<'_>, value: Returned<'_>);
    safe fn zif_ssh2_publickey_remove(frame: Entered<'_>, value: Returned<'_>);
    safe fn zif_ssh2_publickey_list(frame: Entered<'_>, value: Returned<'_>);
    safe fn zif_ssh2_auth_agent(frame: Entered<'_>, value: Returned<'_>);
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

/// The complete table, in upstream's order: the optional families the build
/// has sit between the head and the tracer's functions.
pub(crate) fn functions() -> *const zend_function_entry {
    Table::join(&[
        HEAD.table(),
        #[cfg(feature = "ssh2")]
        SSH2_FUNCTIONS.table(),
        TAIL.table(),
    ])
    .as_ptr()
}

/// MINIT's `go` and `defer` aliases when short names are on.
/// The token is the module's MINIT, which Zend runs after registering the
/// module's functions.
pub(crate) fn register_aliases(
    minit: &swoole_rs_zend::Minit,
    flags: u32,
) -> Result<(), swoole_rs_zend::bailout::Bailout> {
    if flags & SHORTNAME == 0 {
        return Ok(());
    }
    minit.function_alias(
        c"swoole_coroutine_create",
        c"go",
        &s::arginfo_swoole_coroutine_create,
    )?;
    minit.function_alias(
        c"swoole_coroutine_defer",
        c"defer",
        &s::arginfo_swoole_coroutine_defer,
    )
}
