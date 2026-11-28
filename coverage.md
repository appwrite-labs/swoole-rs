# Coverage of crates/

Lines of `crates/` the Cargo swoole.so ran under the PHPT corpus, the corpus apps and swoole/library's suite, measured on `341aeae` (`tools/coverage.py`, nightly `.github/workflows/coverage.yml`). A process that exits by a signal or `_exit` records nothing.

| crate | lines | covered | entry points not reached |
| --- | ---: | ---: | ---: |
| crc32 | 41 | 0.0% | 0 |
| curl-sys | 10 | 0.0% | 0 |
| deflate | 61 | 0.0% | 0 |
| firebird-sys | 12 | 0.0% | 0 |
| phpthread | 1,647 | 0.0% | 52 |
| protocols | 390 | 0.0% | 0 |
| ssh2 | 1,672 | 0.0% | 35 |
| firebird | 1,619 | 1.2% | 0 |
| odbc | 1,025 | 3.1% | 0 |
| curl | 2,444 | 4.4% | 28 |
| phpredis | 195 | 9.2% | 3 |
| bus | 559 | 11.3% | 0 |
| phpevents | 741 | 13.4% | 9 |
| clients | 2,747 | 14.8% | 1 |
| http | 4,212 | 21.7% | 0 |
| inflate | 133 | 23.3% | 0 |
| base64 | 84 | 35.7% | 0 |
| server | 10,823 | 36.1% | 0 |
| pgsql | 2,156 | 36.3% | 9 |
| error | 120 | 38.3% | 0 |
| phpclients | 7,172 | 40.1% | 61 |
| ports | 1,544 | 40.1% | 0 |
| phpprocess | 2,006 | 40.9% | 26 |
| error-ffi | 114 | 42.1% | 6 |
| phpruntime | 2,618 | 44.2% | 8 |
| phpcoroutines | 2,852 | 48.1% | 21 |
| corebase | 22,027 | 48.6% | 5 |
| hooks | 4,374 | 49.3% | 5 |
| phpserver | 4,599 | 49.4% | 29 |
| phplocks | 442 | 49.5% | 20 |
| sqlite | 471 | 53.3% | 0 |
| oci | 1,452 | 53.8% | 0 |
| phpswoole | 1,621 | 57.1% | 13 |
| phphttp | 5,889 | 57.5% | 8 |
| zlib | 114 | 57.9% | 0 |
| framing | 361 | 58.7% | 0 |
| locale | 116 | 62.1% | 0 |
| zend | 3,768 | 64.6% | 8 |
| websocket | 404 | 69.1% | 0 |
| phphttp2 | 1,163 | 71.4% | 3 |
| httpbindings | 239 | 75.3% | 0 |
| phptimer | 165 | 75.8% | 1 |
| http2 | 776 | 77.7% | 0 |
| phpcollections | 557 | 83.1% | 0 |
| brotli | 85 | 87.1% | 0 |
| ext | 3 | 100.0% | 0 |
| ffi | 20 | 100.0% | 0 |
| phpstubs | 12 | 100.0% | 0 |
| **crates/** | 95,655 | 41.2% | 351 |

# Entry points no run reached

`zif_`/`zim_` handlers and `swr_v1_` exports whose counter stayed zero, per crate: candidates for targeted differential tests ([#2249](https://github.com/appwrite-labs/swoole-rs/issues/2249)).

## clients

- `swr_v1_clients_address_verify`

## corebase

- `swr_v1_core_fatal_error`
- `swr_v1_core_format_result`
- `swr_v1_coroutine_scheduler_get`
- `swr_v1_ffi_exit`
- `swr_v1_logger_put`

## curl

- `swr_v1_curl_create_handle`
- `swr_v1_curl_get_handle`
- `swr_v1_curl_handle_leave_multi`
- `swr_v1_curl_multi_free`
- `zif_swoole_native_curl_close`
- `zif_swoole_native_curl_copy_handle`
- `zif_swoole_native_curl_errno`
- `zif_swoole_native_curl_error`
- `zif_swoole_native_curl_escape`
- `zif_swoole_native_curl_exec`
- `zif_swoole_native_curl_getinfo`
- `zif_swoole_native_curl_init`
- `zif_swoole_native_curl_multi_add_handle`
- `zif_swoole_native_curl_multi_close`
- `zif_swoole_native_curl_multi_errno`
- `zif_swoole_native_curl_multi_exec`
- `zif_swoole_native_curl_multi_getcontent`
- `zif_swoole_native_curl_multi_info_read`
- `zif_swoole_native_curl_multi_init`
- `zif_swoole_native_curl_multi_remove_handle`
- `zif_swoole_native_curl_multi_select`
- `zif_swoole_native_curl_multi_setopt`
- `zif_swoole_native_curl_pause`
- `zif_swoole_native_curl_reset`
- `zif_swoole_native_curl_setopt`
- `zif_swoole_native_curl_setopt_array`
- `zif_swoole_native_curl_unescape`
- `zif_swoole_native_curl_upkeep`

## error-ffi

- `swr_v1_abi_version`
- `swr_v1_error_buffer`
- `swr_v1_error_message`
- `swr_v1_error_resolve`
- `swr_v1_error_strerror`
- `swr_v1_feature_bits`

## hooks

- `swr_v1_hook_co_socket_connect_host`
- `swr_v1_hook_co_socket_new`
- `swr_v1_hook_co_socket_uring`
- `swr_v1_ssh2_bound`
- `swr_v1_ssh2_set_socket_timeout`

## pgsql

- `zim_PDO_PGSql_Ext_pgsqlCopyFromArray`
- `zim_PDO_PGSql_Ext_pgsqlCopyFromFile`
- `zim_PDO_PGSql_Ext_pgsqlCopyToArray`
- `zim_PDO_PGSql_Ext_pgsqlCopyToFile`
- `zim_PDO_PGSql_Ext_pgsqlGetNotify`
- `zim_PDO_PGSql_Ext_pgsqlGetPid`
- `zim_PDO_PGSql_Ext_pgsqlLOBCreate`
- `zim_PDO_PGSql_Ext_pgsqlLOBOpen`
- `zim_PDO_PGSql_Ext_pgsqlLOBUnlink`

## phpclients

- `zif_swoole_client_select`
- `zif_swoole_coroutine_socketpair`
- `zim_swoole_client_async_close`
- `zim_swoole_client_async_connect`
- `zim_swoole_client_async_enableSSL`
- `zim_swoole_client_async_isConnected`
- `zim_swoole_client_async_sleep`
- `zim_swoole_client_async_wakeup`
- `zim_swoole_client_close`
- `zim_swoole_client_connect`
- `zim_swoole_client_coro_close`
- `zim_swoole_client_coro_enableSSL`
- `zim_swoole_client_coro_exportSocket`
- `zim_swoole_client_coro_getpeername`
- `zim_swoole_client_coro_getsockname`
- `zim_swoole_client_coro_isConnected`
- `zim_swoole_client_coro_recv`
- `zim_swoole_client_coro_recvfrom`
- `zim_swoole_client_coro_sendfile`
- `zim_swoole_client_coro_sendto`
- `zim_swoole_client_coro_set`
- `zim_swoole_client_enableSSL`
- `zim_swoole_client_getSocket`
- `zim_swoole_client_getpeername`
- `zim_swoole_client_getsockname`
- `zim_swoole_client_recv`
- `zim_swoole_client_send`
- `zim_swoole_client_sendfile`
- `zim_swoole_client_sendto`
- `zim_swoole_client_set`
- `zim_swoole_client_shutdown`
- `zim_swoole_http_client_coro_disconnect`
- `zim_swoole_http_client_coro_download`
- `zim_swoole_http_client_coro_getHeaderOut`
- `zim_swoole_http_client_coro_getPeerCert`
- `zim_swoole_http_client_coro_ping`
- `zim_swoole_http_client_coro_recv`
- `zim_swoole_http_client_coro_setBasicAuth`
- `zim_swoole_socket_coro_checkLiveness`
- `zim_swoole_socket_coro_getBoundCid`
- `zim_swoole_socket_coro_getOption`
- `zim_swoole_socket_coro_getpeername`
- `zim_swoole_socket_coro_import`
- `zim_swoole_socket_coro_isClosed`
- `zim_swoole_socket_coro_peek`
- `zim_swoole_socket_coro_readVector`
- `zim_swoole_socket_coro_readVectorAll`
- `zim_swoole_socket_coro_recvAll`
- `zim_swoole_socket_coro_recvLine`
- `zim_swoole_socket_coro_recvWithBuffer`
- `zim_swoole_socket_coro_recvfrom`
- `zim_swoole_socket_coro_send`
- `zim_swoole_socket_coro_sendFile`
- `zim_swoole_socket_coro_sendto`
- `zim_swoole_socket_coro_setOption`
- `zim_swoole_socket_coro_shutdown`
- `zim_swoole_socket_coro_sslHandshake`
- `zim_swoole_socket_coro_writeVector`
- `zim_swoole_socket_coro_writeVectorAll`
- `zim_swoole_websocket_frame___toString`
- `zim_swoole_websocket_server_pack`

## phpcoroutines

- `zif_swoole_clear_dns_cache`
- `zif_swoole_coroutine_defer`
- `zif_swoole_exit`
- `zif_swoole_test_kernel_coroutine`
- `zim_swoole_coroutine_cancel`
- `zim_swoole_coroutine_disableScheduler`
- `zim_swoole_coroutine_enableScheduler`
- `zim_swoole_coroutine_exists`
- `zim_swoole_coroutine_getBackTrace`
- `zim_swoole_coroutine_getElapsed`
- `zim_swoole_coroutine_getExecuteTime`
- `zim_swoole_coroutine_isCanceled`
- `zim_swoole_coroutine_printBackTrace`
- `zim_swoole_coroutine_scheduler_parallel`
- `zim_swoole_coroutine_setTimeLimit`
- `zim_swoole_coroutine_system_getaddrinfo`
- `zim_swoole_coroutine_system_statvfs`
- `zim_swoole_coroutine_system_waitEvent`
- `zim_swoole_coroutine_system_waitSignal`
- `zim_swoole_exit_exception_getFlags`
- `zim_swoole_exit_exception_getStatus`

## phpevents

- `zif_swoole_event_add`
- `zif_swoole_event_cycle`
- `zif_swoole_event_defer`
- `zif_swoole_event_del`
- `zif_swoole_event_dispatch`
- `zif_swoole_event_exit`
- `zif_swoole_event_isset`
- `zif_swoole_event_set`
- `zif_swoole_event_write`

## phphttp

- `zim_swoole_http_response_create`
- `zim_swoole_http_response_goaway`
- `zim_swoole_http_response_ping`
- `zim_swoole_http_response_push`
- `zim_swoole_http_response_sendfile`
- `zim_swoole_websocket_server_disconnect`
- `zim_swoole_websocket_server_isEstablished`
- `zim_swoole_websocket_server_ping`

## phphttp2

- `zim_swoole_http2_client_coro_goaway`
- `zim_swoole_http2_client_coro_read`
- `zim_swoole_http2_client_coro_write`

## phplocks

- `zim_swoole_coroutine_lock___construct`
- `zim_swoole_coroutine_lock_lock`
- `zim_swoole_coroutine_lock_unlock`
- `zim_swoole_thread_atomic___construct`
- `zim_swoole_thread_atomic_add`
- `zim_swoole_thread_atomic_cmpset`
- `zim_swoole_thread_atomic_get`
- `zim_swoole_thread_atomic_long___construct`
- `zim_swoole_thread_atomic_long_add`
- `zim_swoole_thread_atomic_long_cmpset`
- `zim_swoole_thread_atomic_long_get`
- `zim_swoole_thread_atomic_long_set`
- `zim_swoole_thread_atomic_long_sub`
- `zim_swoole_thread_atomic_set`
- `zim_swoole_thread_atomic_sub`
- `zim_swoole_thread_atomic_wait`
- `zim_swoole_thread_atomic_wakeup`
- `zim_swoole_thread_lock___construct`
- `zim_swoole_thread_lock_lock`
- `zim_swoole_thread_lock_unlock`

## phpprocess

- `zim_swoole_process_alarm`
- `zim_swoole_process_close`
- `zim_swoole_process_daemon`
- `zim_swoole_process_exec`
- `zim_swoole_process_exit`
- `zim_swoole_process_exportSocket`
- `zim_swoole_process_freeQueue`
- `zim_swoole_process_getAffinity`
- `zim_swoole_process_getPriority`
- `zim_swoole_process_pool_detach`
- `zim_swoole_process_pool_getProcess`
- `zim_swoole_process_pool_listen`
- `zim_swoole_process_pool_shutdown`
- `zim_swoole_process_pool_stop`
- `zim_swoole_process_pool_write`
- `zim_swoole_process_pop`
- `zim_swoole_process_push`
- `zim_swoole_process_read`
- `zim_swoole_process_set`
- `zim_swoole_process_setAffinity`
- `zim_swoole_process_setBlocking`
- `zim_swoole_process_setPriority`
- `zim_swoole_process_setTimeout`
- `zim_swoole_process_statQueue`
- `zim_swoole_process_useQueue`
- `zim_swoole_process_write`

## phpredis

- `zim_swoole_redis_server_format`
- `zim_swoole_redis_server_getHandler`
- `zim_swoole_redis_server_setHandler`

## phpruntime

- `swr_v1_phpruntime_socket_cast`
- `swr_v1_phpruntime_socket_stat`
- `zif_swoole_proc_terminate`
- `zif_swoole_putenv`
- `zif_swoole_stream_select`
- `zif_swoole_stream_socket_pair`
- `zif_swoole_time_nanosleep`
- `zif_swoole_time_sleep_until`

## phpserver

- `zim_swoole_connection_iterator_current`
- `zim_swoole_connection_iterator_key`
- `zim_swoole_connection_iterator_next`
- `zim_swoole_server_addCommand`
- `zim_swoole_server_close`
- `zim_swoole_server_command`
- `zim_swoole_server_finish`
- `zim_swoole_server_getManagerPid`
- `zim_swoole_server_getMasterPid`
- `zim_swoole_server_getSocket`
- `zim_swoole_server_getWorkerId`
- `zim_swoole_server_getWorkerPid`
- `zim_swoole_server_getWorkerStatus`
- `zim_swoole_server_pause`
- `zim_swoole_server_port___construct`
- `zim_swoole_server_reload`
- `zim_swoole_server_resume`
- `zim_swoole_server_sendMessage`
- `zim_swoole_server_sendfile`
- `zim_swoole_server_sendto`
- `zim_swoole_server_sendwait`
- `zim_swoole_server_stats`
- `zim_swoole_server_stop`
- `zim_swoole_server_task`
- `zim_swoole_server_taskCo`
- `zim_swoole_server_taskWaitMulti`
- `zim_swoole_server_task_finish`
- `zim_swoole_server_task_pack`
- `zim_swoole_server_task_unpack`

## phpswoole

- `zif_swoole_async_dns_lookup_coro`
- `zif_swoole_async_set`
- `zif_swoole_clear_error`
- `zif_swoole_errno`
- `zif_swoole_error_log_ex`
- `zif_swoole_get_local_ip`
- `zif_swoole_get_local_mac`
- `zif_swoole_ignore_error`
- `zif_swoole_set_process_name`
- `zif_swoole_strerror`
- `zif_swoole_substr_json_decode`
- `zif_swoole_substr_unserialize`
- `zif_swoole_version`

## phpthread

- `zim_swoole_thread___construct`
- `zim_swoole_thread_activeCount`
- `zim_swoole_thread_arraylist___construct`
- `zim_swoole_thread_arraylist_clean`
- `zim_swoole_thread_arraylist_count`
- `zim_swoole_thread_arraylist_decr`
- `zim_swoole_thread_arraylist_find`
- `zim_swoole_thread_arraylist_incr`
- `zim_swoole_thread_arraylist_offsetExists`
- `zim_swoole_thread_arraylist_offsetGet`
- `zim_swoole_thread_arraylist_offsetSet`
- `zim_swoole_thread_arraylist_offsetUnset`
- `zim_swoole_thread_arraylist_sort`
- `zim_swoole_thread_arraylist_toArray`
- `zim_swoole_thread_barrier___construct`
- `zim_swoole_thread_barrier_wait`
- `zim_swoole_thread_detach`
- `zim_swoole_thread_getAffinity`
- `zim_swoole_thread_getArguments`
- `zim_swoole_thread_getExitStatus`
- `zim_swoole_thread_getId`
- `zim_swoole_thread_getInfo`
- `zim_swoole_thread_getNativeId`
- `zim_swoole_thread_getPriority`
- `zim_swoole_thread_isAlive`
- `zim_swoole_thread_join`
- `zim_swoole_thread_joinable`
- `zim_swoole_thread_map___construct`
- `zim_swoole_thread_map_add`
- `zim_swoole_thread_map_clean`
- `zim_swoole_thread_map_count`
- `zim_swoole_thread_map_decr`
- `zim_swoole_thread_map_find`
- `zim_swoole_thread_map_incr`
- `zim_swoole_thread_map_keys`
- `zim_swoole_thread_map_offsetExists`
- `zim_swoole_thread_map_offsetGet`
- `zim_swoole_thread_map_offsetSet`
- `zim_swoole_thread_map_offsetUnset`
- `zim_swoole_thread_map_sort`
- `zim_swoole_thread_map_toArray`
- `zim_swoole_thread_map_update`
- `zim_swoole_thread_map_values`
- `zim_swoole_thread_queue___construct`
- `zim_swoole_thread_queue_clean`
- `zim_swoole_thread_queue_count`
- `zim_swoole_thread_queue_pop`
- `zim_swoole_thread_queue_push`
- `zim_swoole_thread_setAffinity`
- `zim_swoole_thread_setName`
- `zim_swoole_thread_setPriority`
- `zim_swoole_thread_yield`

## phptimer

- `zif_swoole_timer_info`

## ssh2

- `zif_ssh2_auth_agent`
- `zif_ssh2_auth_hostbased_file`
- `zif_ssh2_auth_none`
- `zif_ssh2_auth_password`
- `zif_ssh2_auth_pubkey`
- `zif_ssh2_auth_pubkey_file`
- `zif_ssh2_connect`
- `zif_ssh2_disconnect`
- `zif_ssh2_exec`
- `zif_ssh2_fetch_stream`
- `zif_ssh2_fingerprint`
- `zif_ssh2_forward_accept`
- `zif_ssh2_forward_listen`
- `zif_ssh2_methods_negotiated`
- `zif_ssh2_publickey_add`
- `zif_ssh2_publickey_init`
- `zif_ssh2_publickey_list`
- `zif_ssh2_publickey_remove`
- `zif_ssh2_scp_recv`
- `zif_ssh2_scp_send`
- `zif_ssh2_send_eof`
- `zif_ssh2_sftp`
- `zif_ssh2_sftp_chmod`
- `zif_ssh2_sftp_lstat`
- `zif_ssh2_sftp_mkdir`
- `zif_ssh2_sftp_readlink`
- `zif_ssh2_sftp_realpath`
- `zif_ssh2_sftp_rename`
- `zif_ssh2_sftp_rmdir`
- `zif_ssh2_sftp_stat`
- `zif_ssh2_sftp_symlink`
- `zif_ssh2_sftp_unlink`
- `zif_ssh2_shell`
- `zif_ssh2_shell_resize`
- `zif_ssh2_tunnel`

## zend

- `swr_v1_curl_check_handle`
- `swr_v1_phphelpers_class_alias`
- `swr_v1_phpruntime_plain_fdopen_mode`
- `swr_v1_phpruntime_plain_metadata`
- `swr_v1_phpruntime_plain_rename`
- `swr_v1_phpruntime_plain_rmdir`
- `swr_v1_phpruntime_stream_from_socket`
- `swr_v1_phpswoole_fatal_exit`
