# Coverage of crates/

Lines of `crates/` the Cargo swoole.so ran under the PHPT corpus, the corpus apps and swoole/library's suite, measured on `ff3f947` (`tools/coverage.py`, nightly `.github/workflows/coverage.yml`). A process that exits by a signal or `_exit` records nothing.

| crate | lines | covered | entry points not reached |
| --- | ---: | ---: | ---: |
| phpthread | 1,927 | 0.0% | 52 |
| websocket-ffi | 240 | 22.9% | 8 |
| protocols-ffi | 535 | 29.7% | 15 |
| utilities | 999 | 31.4% | 23 |
| base64 | 99 | 33.3% | 0 |
| crc32 | 42 | 45.2% | 0 |
| odbc | 237 | 49.8% | 0 |
| memory | 2,068 | 51.0% | 1 |
| ossupport | 943 | 51.9% | 24 |
| phplocks | 552 | 52.2% | 22 |
| http-ffi | 922 | 59.8% | 12 |
| buffer | 878 | 60.5% | 2 |
| corebase | 2,044 | 61.6% | 8 |
| firebird | 51 | 66.7% | 0 |
| protocols | 449 | 67.3% | 0 |
| error | 132 | 68.2% | 3 |
| phpredis | 258 | 68.6% | 0 |
| coroutine | 1,029 | 68.7% | 0 |
| phpevents | 831 | 69.3% | 0 |
| ssh2 | 122 | 71.3% | 0 |
| locks | 448 | 71.4% | 4 |
| phpcoroutines | 2,843 | 71.9% | 6 |
| phpswoole | 1,742 | 71.9% | 10 |
| timer | 1,137 | 72.7% | 12 |
| server | 11,386 | 74.0% | 16 |
| process | 5,013 | 74.7% | 1 |
| clients | 2,310 | 74.9% | 11 |
| tls | 2,678 | 76.7% | 5 |
| phpprocess | 2,065 | 77.6% | 2 |
| phpruntime | 2,493 | 77.7% | 1 |
| http | 2,586 | 77.8% | 0 |
| ports | 2,114 | 78.0% | 3 |
| hooks | 5,109 | 78.5% | 16 |
| phphelpers | 517 | 79.7% | 1 |
| zend | 1,789 | 79.9% | 0 |
| phpserver | 4,874 | 80.0% | 11 |
| reactor | 3,118 | 80.5% | 5 |
| pgsql | 382 | 81.2% | 0 |
| phpclients | 7,081 | 81.9% | 10 |
| sqlite | 74 | 83.8% | 0 |
| phphttp2 | 2,737 | 83.8% | 2 |
| phpcollections | 725 | 83.9% | 2 |
| phptimer | 212 | 84.9% | 0 |
| inflate | 100 | 86.0% | 0 |
| httpbindings | 580 | 87.6% | 0 |
| framing | 356 | 87.9% | 0 |
| brotli | 58 | 87.9% | 0 |
| phphttp | 5,559 | 88.0% | 4 |
| deflate | 56 | 89.3% | 0 |
| http2 | 330 | 89.7% | 0 |
| ffi | 51 | 90.2% | 1 |
| bus | 444 | 90.3% | 0 |
| curl | 634 | 92.0% | 2 |
| zlib | 105 | 93.3% | 0 |
| websocket | 312 | 95.5% | 0 |
| oci | 124 | 99.2% | 0 |
| curl-sys | 4 | 100.0% | 0 |
| ext | 3 | 100.0% | 0 |
| locale | 39 | 100.0% | 0 |
| **crates/** | 86,546 | 73.6% | 295 |

# Entry points no run reached

`zif_`/`zim_` handlers and `swr_v1_` exports whose counter stayed zero, per crate: candidates for targeted differential tests ([#2249](https://github.com/appwrite-labs/swoole-rs/issues/2249)).

## buffer

- `swr_v1_buffer_iovec_layout`
- `swr_v1_string_split`

## clients

- `swr_v1_clients_address_empty`
- `swr_v1_clients_address_set_port`
- `swr_v1_clients_address_type`
- `swr_v1_clients_dns_server`
- `swr_v1_clients_flag`
- `swr_v1_clients_hosts_lookup`
- `swr_v1_clients_load_resolv_conf`
- `swr_v1_clients_proxy_fields`
- `swr_v1_clients_set_hosts_path`
- `swr_v1_clients_url`
- `swr_v1_clients_watermark`

## corebase

- `swr_v1_core_format_result`
- `swr_v1_core_proxy_assign`
- `swr_v1_core_resolver_resize`
- `swr_v1_core_signal_alarm`
- `swr_v1_core_string_clone`
- `swr_v1_core_string_free`
- `swr_v1_core_trace_flags`
- `swr_v1_corebase_parse_stoi`

## curl

- `swr_v1_curl_multi_co`
- `swr_v1_curl_multi_exec`

## error

- `swr_v1_error_ignored_release`
- `swr_v1_error_message`
- `swr_v1_error_resolve`

## ffi

- `swr_v1_ffi_exit`

## hooks

- `swr_v1_hook_co_socket_cancel`
- `swr_v1_hook_co_socket_check_liveness`
- `swr_v1_hook_co_socket_close`
- `swr_v1_hook_co_socket_event_str`
- `swr_v1_hook_co_socket_free`
- `swr_v1_hook_co_socket_make`
- `swr_v1_hook_co_socket_owned_err`
- `swr_v1_hook_co_socket_proxy`
- `swr_v1_hook_co_socket_shutdown`
- `swr_v1_hook_co_socket_timed_out`
- `swr_v1_hook_co_socket_uring`
- `swr_v1_hook_socket_poll`
- `swr_v1_hook_timeout_restore`
- `swr_v1_hook_timeout_scope`
- `swr_v1_system_read_file`
- `swr_v1_system_wait_signal`

## http-ffi

- `swr_v1_http_boundary`
- `swr_v1_http_cookie_next`
- `swr_v1_http_method`
- `swr_v1_http_method_string`
- `swr_v1_http_multipart_disposition`
- `swr_v1_http_multipart_filename`
- `swr_v1_http_multipart_result`
- `swr_v1_http_proxy_owned_authorization`
- `swr_v1_http_static_location`
- `swr_v1_http_url_capacity`
- `swr_v1_http_url_decode`
- `swr_v1_http_url_encode`

## locks

- `swr_v1_locks_atomic32`
- `swr_v1_locks_atomic64`
- `swr_v1_locks_coroutine_release`
- `swr_v1_locks_coroutine_try`

## memory

- `swr_v1_memory_global_memory_new`

## ossupport

- `swr_v1_os_async_file_assign`
- `swr_v1_os_async_file_clone`
- `swr_v1_os_async_file_fd`
- `swr_v1_os_async_file_free`
- `swr_v1_os_async_file_new`
- `swr_v1_os_async_file_offset`
- `swr_v1_os_async_file_open`
- `swr_v1_os_async_file_read`
- `swr_v1_os_async_file_stat`
- `swr_v1_os_async_file_sync`
- `swr_v1_os_async_file_truncate`
- `swr_v1_os_async_file_write`
- `swr_v1_os_file_get_contents`
- `swr_v1_os_file_read_content`
- `swr_v1_os_file_record_assign`
- `swr_v1_os_file_record_clone`
- `swr_v1_os_file_record_close`
- `swr_v1_os_file_record_control`
- `swr_v1_os_file_record_fd`
- `swr_v1_os_file_record_free`
- `swr_v1_os_file_record_io`
- `swr_v1_os_file_record_new`
- `swr_v1_os_file_record_open`
- `swr_v1_os_set_isolation`

## phpclients

- `zif_swoole_name_resolver_add`
- `zif_swoole_name_resolver_remove`
- `zim_swoole_client_coro_getPeerCert`
- `zim_swoole_client_coro_peek`
- `zim_swoole_client_coro_verifyPeerCert`
- `zim_swoole_client_getPeerCert`
- `zim_swoole_client_isConnected`
- `zim_swoole_client_verifyPeerCert`
- `zim_swoole_http_client_coro_getDefer`
- `zim_swoole_http_client_coro_getpeername`

## phpcollections

- `zim_swoole_channel_coro_isFull`
- `zim_swoole_table_getSize`

## phpcoroutines

- `zif_swoole_exit`
- `zif_swoole_tracer_leak_detect`
- `zif_swoole_tracer_prof_begin`
- `zif_swoole_tracer_prof_end`
- `zim_swoole_coroutine_getExecuteTime`
- `zim_swoole_coroutine_getStackUsage`

## phphelpers

- `swr_v1_phphelpers_zval_dup`

## phphttp

- `zim_swoole_http_cookie_toString`
- `zim_swoole_http_response_disconnect`
- `zim_swoole_http_response_initHeader`
- `zim_swoole_websocket_server_unpack`

## phphttp2

- `zim_swoole_http2_client_coro_isStreamExist`
- `zim_swoole_http2_client_coro_stats`

## phplocks

- `swr_v1_phplocks_thread_atomic_minit`
- `swr_v1_phplocks_thread_lock_minit`
- `zim_swoole_atomic_long_cmpset`
- `zim_swoole_atomic_long_set`
- `zim_swoole_atomic_long_sub`
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

- `zim_swoole_process_exec`
- `zim_swoole_process_pool_stop`

## phpruntime

- `zif_swoole_putenv`

## phpserver

- `zim_swoole_connection_iterator___construct`
- `zim_swoole_connection_iterator_offsetExists`
- `zim_swoole_connection_iterator_offsetGet`
- `zim_swoole_connection_iterator_offsetSet`
- `zim_swoole_connection_iterator_offsetUnset`
- `zim_swoole_server_bind`
- `zim_swoole_server_getClientList`
- `zim_swoole_server_heartbeat`
- `zim_swoole_server_port___construct`
- `zim_swoole_server_port_getSocket`
- `zim_swoole_server_protect`

## phpswoole

- `zif_swoole_get_object_by_handle`
- `zif_swoole_get_objects`
- `zif_swoole_get_vm_status`
- `zif_swoole_hashcode`
- `zif_swoole_implicit_fn`
- `zif_swoole_mime_type_delete`
- `zif_swoole_mime_type_exists`
- `zif_swoole_mime_type_get`
- `zif_swoole_mime_type_list`
- `zif_swoole_mime_type_set`

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

## ports

- `swr_v1_ports_protocol`
- `swr_v1_ports_protocol_name`
- `swr_v1_ports_set_eof`

## process

- `swr_v1_process_pool_ipc_send_sync`

## protocols-ffi

- `swr_v1_resp_extend_size`
- `swr_v1_resp_format`
- `swr_v1_resp_handler_key`
- `swr_v1_resp_parse`
- `swr_v1_resp_parse_range`
- `swr_v1_resp_php_next`
- `swr_v1_resp_receive`
- `swr_v1_resp_receive_range`
- `swr_v1_resp_scan`
- `swr_v1_socks_auth`
- `swr_v1_socks_config`
- `swr_v1_socks_connect`
- `swr_v1_socks_negotiate`
- `swr_v1_socks_strerror`
- `swr_v1_socks_transition`

## reactor

- `swr_v1_event_remove_exit_condition`
- `swr_v1_reactor_iov_advance`
- `swr_v1_reactor_iov_length`
- `swr_v1_reactor_isset_exit`
- `swr_v1_reactor_registry_get`

## server

- `swr_v1_server_admin_register`
- `swr_v1_server_connection`
- `swr_v1_server_port_shared_init`
- `swr_v1_server_record_layout`
- `swr_v1_server_run`
- `swr_v1_server_session`
- `swr_v1_server_set_document_root`
- `swr_v1_server_startup_error`
- `swr_v1_server_static_mime`
- `swr_v1_server_static_owner`
- `swr_v1_server_static_owner_clone`
- `swr_v1_server_static_owner_free`
- `swr_v1_server_static_rewrite`
- `swr_v1_server_static_rewrite_add`
- `swr_v1_server_static_text`
- `swr_v1_server_text_slot`

## timer

- `swr_v1_timer_get`
- `swr_v1_timer_ids`
- `swr_v1_timer_owner_abandon`
- `swr_v1_timer_owner_add`
- `swr_v1_timer_owner_delay`
- `swr_v1_timer_owner_new`
- `swr_v1_timer_owner_queue`
- `swr_v1_timer_owner_remove`
- `swr_v1_timer_owner_select`
- `swr_v1_timer_owner_update`
- `swr_v1_timer_token`
- `swr_v1_timer_update`

## tls

- `swr_v1_tls_set_capath`
- `swr_v1_tls_set_ciphers`
- `swr_v1_tls_set_client_certificate`
- `swr_v1_tls_set_dhparam`
- `swr_v1_tls_set_ecdh_curve`

## utilities

- `swr_v1_logger_close`
- `swr_v1_logger_file`
- `swr_v1_logger_free`
- `swr_v1_logger_is_opened`
- `swr_v1_logger_level`
- `swr_v1_logger_new`
- `swr_v1_logger_redirect`
- `swr_v1_logger_reopen`
- `swr_v1_logger_rotation_date`
- `swr_v1_logger_set`
- `swr_v1_logger_set_date_format`
- `swr_v1_logger_set_level`
- `swr_v1_logger_set_stream`
- `swr_v1_utils_hash`
- `swr_v1_utils_log_label`
- `swr_v1_utils_log_micros`
- `swr_v1_utils_log_rotation`
- `swr_v1_utils_pretty_range`
- `swr_v1_utils_strlchr`
- `swr_v1_utils_wait`
- `swr_v1_utils_wait_registry_init`
- `swr_v1_utils_wait_registry_size`
- `swr_v1_utils_wakeup`

## websocket-ffi

- `swr_v1_ws_control_invalid`
- `swr_v1_ws_decide`
- `swr_v1_ws_decode`
- `swr_v1_ws_encode_close`
- `swr_v1_ws_encoded_size`
- `swr_v1_ws_mask`
- `swr_v1_ws_message_policy`
- `swr_v1_ws_pack_policy`
