# Coverage of crates/

Lines of `crates/` the Cargo swoole.so ran under the PHPT corpus, the corpus apps and swoole/library's suite, measured on `72bc36e` (`tools/coverage.py`, nightly `.github/workflows/coverage.yml`). A process that exits by a signal or `_exit` records nothing.

| crate | lines | covered | entry points not reached |
| --- | ---: | ---: | ---: |
| phpthread | 1,674 | 0.0% | 52 |
| websocket-ffi | 250 | 9.6% | 9 |
| protocols-ffi | 494 | 29.1% | 16 |
| utilities | 943 | 29.8% | 18 |
| base64 | 99 | 33.3% | 0 |
| error-ffi | 84 | 39.3% | 7 |
| crc32 | 42 | 45.2% | 0 |
| corebase | 2,371 | 60.4% | 23 |
| phplocks | 468 | 60.7% | 17 |
| http-ffi | 738 | 64.1% | 6 |
| buffer | 870 | 64.4% | 2 |
| odbc | 238 | 65.5% | 0 |
| protocols | 449 | 66.1% | 0 |
| phpevents | 730 | 67.1% | 0 |
| phpredis | 204 | 67.6% | 0 |
| locks | 489 | 69.7% | 6 |
| ssh2 | 118 | 70.3% | 0 |
| clients | 2,292 | 70.9% | 16 |
| ossupport | 756 | 71.4% | 2 |
| coroutine | 1,147 | 72.3% | 0 |
| phpcoroutines | 2,848 | 73.6% | 2 |
| phpruntime | 2,154 | 74.4% | 1 |
| process | 5,099 | 74.5% | 2 |
| server | 11,374 | 74.5% | 15 |
| tls | 2,716 | 75.0% | 3 |
| http | 3,980 | 75.4% | 0 |
| memory | 1,681 | 75.6% | 0 |
| ports | 1,866 | 76.3% | 4 |
| phpprocess | 2,033 | 76.3% | 2 |
| error | 96 | 77.1% | 0 |
| timer | 1,000 | 77.2% | 3 |
| reactor | 3,470 | 78.4% | 13 |
| phpswoole | 1,676 | 80.0% | 0 |
| hooks | 4,804 | 81.3% | 1 |
| pgsql | 381 | 81.4% | 0 |
| firebird | 51 | 82.4% | 0 |
| phpclients | 6,637 | 82.5% | 0 |
| phptimer | 172 | 83.1% | 0 |
| phpserver | 4,690 | 83.3% | 1 |
| zend | 2,904 | 83.7% | 1 |
| sqlite | 74 | 83.8% | 0 |
| inflate | 100 | 86.0% | 0 |
| phpcollections | 572 | 86.0% | 0 |
| brotli | 58 | 87.9% | 0 |
| zlib | 108 | 88.0% | 0 |
| phphttp2 | 2,309 | 88.0% | 0 |
| httpbindings | 301 | 88.7% | 0 |
| phphttp | 5,177 | 88.7% | 0 |
| bus | 442 | 89.6% | 0 |
| framing | 374 | 90.4% | 0 |
| deflate | 54 | 90.7% | 0 |
| http2 | 710 | 91.3% | 0 |
| ffi | 61 | 91.8% | 1 |
| curl | 629 | 92.4% | 0 |
| websocket | 312 | 95.5% | 0 |
| oci | 124 | 97.6% | 0 |
| curl-sys | 4 | 100.0% | 0 |
| ext | 3 | 100.0% | 0 |
| locale | 46 | 100.0% | 0 |
| **crates/** | 85,546 | 75.3% | 223 |

# Entry points no run reached

`zif_`/`zim_` handlers and `swr_v1_` exports whose counter stayed zero, per crate: candidates for targeted differential tests ([#2249](https://github.com/appwrite-labs/swoole-rs/issues/2249)).

## buffer

- `swr_v1_buffer_iovec_layout`
- `swr_v1_string_split`

## clients

- `swr_v1_clients_address_assign`
- `swr_v1_clients_address_empty`
- `swr_v1_clients_address_set_port`
- `swr_v1_clients_address_string`
- `swr_v1_clients_address_type`
- `swr_v1_clients_dns_server`
- `swr_v1_clients_flag`
- `swr_v1_clients_hosts_lookup`
- `swr_v1_clients_http_format`
- `swr_v1_clients_load_resolv_conf`
- `swr_v1_clients_proxy_fields`
- `swr_v1_clients_set_dns_server`
- `swr_v1_clients_set_hosts_path`
- `swr_v1_clients_ssl_context`
- `swr_v1_clients_url`
- `swr_v1_clients_watermark`

## corebase

- `swr_v1_clients_gethostbyname`
- `swr_v1_core_format_result`
- `swr_v1_core_last_error`
- `swr_v1_core_proxy_assign`
- `swr_v1_core_proxy_configure`
- `swr_v1_core_proxy_new`
- `swr_v1_core_resolver_assign`
- `swr_v1_core_resolver_create`
- `swr_v1_core_resolver_finish`
- `swr_v1_core_resolver_host`
- `swr_v1_core_resolver_lookup`
- `swr_v1_core_resolver_new`
- `swr_v1_core_resolver_next`
- `swr_v1_core_resolver_resize`
- `swr_v1_core_resolver_span`
- `swr_v1_core_set_task_tmpdir`
- `swr_v1_core_signal_alarm`
- `swr_v1_core_string_clone`
- `swr_v1_core_string_free`
- `swr_v1_core_string_set`
- `swr_v1_core_string_view`
- `swr_v1_core_trace_flags`
- `swr_v1_corebase_parse_stoi`

## error-ffi

- `swr_v1_abi_version`
- `swr_v1_error_buffer`
- `swr_v1_error_ignored_release`
- `swr_v1_error_message`
- `swr_v1_error_resolve`
- `swr_v1_error_strerror`
- `swr_v1_feature_bits`

## ffi

- `swr_v1_ffi_exit`

## hooks

- `swr_v1_hook_co_socket_uring`

## http-ffi

- `swr_v1_http_boundary`
- `swr_v1_http_method`
- `swr_v1_http_method_string`
- `swr_v1_http_multipart_disposition`
- `swr_v1_http_multipart_filename`
- `swr_v1_http_static_location`

## locks

- `swr_v1_locks_atomic32`
- `swr_v1_locks_atomic64`
- `swr_v1_locks_coroutine_release`
- `swr_v1_locks_coroutine_try`
- `swr_v1_locks_wait`
- `swr_v1_locks_wakeup`

## ossupport

- `swr_v1_os_file_record_fd`
- `swr_v1_os_set_isolation`

## phpcoroutines

- `zif_swoole_exit`
- `zim_swoole_coroutine_getExecuteTime`

## phplocks

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

- `zim_swoole_server_port___construct`

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

- `swr_v1_ports_callback_property`
- `swr_v1_ports_event`
- `swr_v1_ports_protocol`
- `swr_v1_ports_protocol_name`

## process

- `swr_v1_process_pool_ipc_send_sync`
- `swr_v1_worker_globals`

## protocols-ffi

- `swr_v1_mqtt_frame_length`
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

- `swr_v1_reactor_deferred_push`
- `swr_v1_reactor_handlers_default`
- `swr_v1_reactor_handlers_get`
- `swr_v1_reactor_iov_advance`
- `swr_v1_reactor_iov_destroy`
- `swr_v1_reactor_iov_length`
- `swr_v1_reactor_iov_prepare`
- `swr_v1_reactor_isset_exit`
- `swr_v1_reactor_registry_count`
- `swr_v1_reactor_registry_get`
- `swr_v1_reactor_set_end`
- `swr_v1_socket_default_timeouts`
- `swr_v1_socket_host_type`

## server

- `swr_v1_server_connection`
- `swr_v1_server_port_shared_init`
- `swr_v1_server_record_layout`
- `swr_v1_server_session`
- `swr_v1_server_set_document_root`
- `swr_v1_server_state_free`
- `swr_v1_server_static_config`
- `swr_v1_server_static_mime`
- `swr_v1_server_static_owner`
- `swr_v1_server_static_owner_clone`
- `swr_v1_server_static_owner_free`
- `swr_v1_server_static_rewrite`
- `swr_v1_server_static_rewrite_add`
- `swr_v1_server_static_text`
- `swr_v1_server_text_slot`

## timer

- `swr_v1_timer_ids`
- `swr_v1_timer_stats_get`
- `swr_v1_timer_token`

## tls

- `swr_v1_tls_owner_clone`
- `swr_v1_tls_owner_free`
- `swr_v1_tls_owner_set_path`

## utilities

- `swr_v1_logger_close`
- `swr_v1_logger_file`
- `swr_v1_logger_free`
- `swr_v1_logger_is_opened`
- `swr_v1_logger_level`
- `swr_v1_logger_new`
- `swr_v1_logger_open`
- `swr_v1_logger_put`
- `swr_v1_logger_redirect`
- `swr_v1_logger_reopen`
- `swr_v1_logger_rotation_date`
- `swr_v1_logger_set`
- `swr_v1_logger_set_date_format`
- `swr_v1_logger_set_level`
- `swr_v1_logger_set_stream`
- `swr_v1_utils_pretty_range`
- `swr_v1_utils_wait`
- `swr_v1_utils_wakeup`

## websocket-ffi

- `swr_v1_ws_control_invalid`
- `swr_v1_ws_decide`
- `swr_v1_ws_decode`
- `swr_v1_ws_encode`
- `swr_v1_ws_encode_close`
- `swr_v1_ws_encoded_size`
- `swr_v1_ws_mask`
- `swr_v1_ws_message_policy`
- `swr_v1_ws_pack_policy`

## zend

- `swr_v1_phphelpers_zval_dup`
