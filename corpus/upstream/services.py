#!/usr/bin/env python3
"""Print the services (services.yml) that PHPT families reach, space separated.

    services.py [FAMILY...]

No family means every family. A family missing from NEEDS gets every service, so a pin
that adds a family can't drop a service its tests need: under a missing service they
would fail or skip on upstream too, and the gate would never see them.

NEEDS comes from the pinned tests/<family>/ trees: config.php's service constants
(MYSQL_SERVER_*, ODBC_DSN, PGSQL_*, REDIS_SERVER_*, HTTPBIN_*, TEST_HTTP2_SERVERPUSH_URL,
HTTP_PROXY_*, SOCKS5_PROXY_*, FTP_*, FIREBIRD_*, ORACLE_*), the include/ helpers that read
them (skip_if_*mysql8, skip_if_not_redis5, skip_if_no_*proxy, skip_if_no_ftp,
build_ftp_url, SwooleTest\\Redis) and curl's PHP_CURL_FTP_REMOTE_SERVER.
"""
import sys

SERVICES = ['httpbin', 'pgsql', 'mysql', 'redis', 'golang-h2demo', 'tinyproxy', 'socks5', 'ftp',
            'firebirdsql', 'oracle']
NONE = ['swoole_atomic', 'swoole_client_async', 'swoole_client_coro', 'swoole_coroutine_lock',
        'swoole_coroutine_scheduler', 'swoole_coroutine_system', 'swoole_coroutine_util',
        'swoole_coroutine_wait_group', 'swoole_event', 'swoole_ftp', 'swoole_function', 'swoole_global',
        'swoole_http2_server', 'swoole_http2_server_coro', 'swoole_http_server_coro', 'swoole_iouring',
        'swoole_library', 'swoole_lock', 'swoole_pdo_sqlite', 'swoole_process', 'swoole_process_pool',
        'swoole_redis_server', 'swoole_server_coro', 'swoole_server_port', 'swoole_ssh2', 'swoole_stdext',
        'swoole_table', 'swoole_test_runner', 'swoole_thread', 'swoole_timer', 'swoole_websocket_server']
NEEDS = {family: [] for family in NONE} | {
    'swoole_channel_coro': ['redis'],
    'swoole_client_sync': ['tinyproxy', 'socks5'],
    'swoole_coroutine': ['mysql', 'redis'],
    'swoole_curl': ['httpbin', 'golang-h2demo', 'ftp'],
    'swoole_feature': ['redis'],
    'swoole_http2_client_coro': ['httpbin', 'tinyproxy'],
    'swoole_http_client_coro': ['httpbin', 'tinyproxy', 'socks5'],
    'swoole_http_server': ['mysql'],
    'swoole_pdo_firebird': ['firebirdsql'],
    'swoole_pdo_odbc': ['mysql'],
    'swoole_pdo_oracle': ['oracle'],
    'swoole_pdo_pgsql': ['pgsql'],
    'swoole_runtime': ['mysql', 'redis', 'ftp'],
    'swoole_server': ['mysql', 'redis'],
    'swoole_socket_coro': ['redis'],
}


def services(families):
    needed = {service for family in families for service in NEEDS.get(family, SERVICES)}
    return [service for service in SERVICES if not families or service in needed]


if __name__ == '__main__':
    print(' '.join(services(sys.argv[1:])))
