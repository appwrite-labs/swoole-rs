#!/usr/bin/env python3
"""Generate the two reserved port overlays from pinned objects; never edit reference."""
import difflib
from pathlib import Path
import subprocess
import textwrap

ROOT = Path(__file__).resolve().parents[3]
SOURCE = ROOT / 'target/upstream/src'
import sys
sys.path.insert(0, str(ROOT))
from tools.build_hybrid import module_patches
from tools.build_upstream import COMMIT as PIN


def original(path):
    return subprocess.check_output(['git', '-C', str(SOURCE), 'show', f'{PIN}:{path}'], text=True)


def replace(text, old, new):
    if text.count(old) != 1:
        raise ValueError('Expected one source occurrence: ' + old[:70])
    return text.replace(old, new)


def body(text, signature, value):
    start = text.index(signature)
    begin = text.index('{', start)
    depth, end = 1, begin + 1
    while depth:
        depth += (text[end] == '{') - (text[end] == '}')
        end += 1
    return text[:begin+1] + '\n' + value.strip('\n') + '\n}' + text[end:]


def ordinary_settings(php):
    start = php.index('    // backlog', php.index('static PHP_METHOD(swoole_server_port, set) {'))
    end = php.index('    if (port->ssl) {', start)
    prefix = php[start:end]
    def effects(key):
        position = prefix.index(f'if (php_swoole_array_get_value(vht, "{key}", ztmp)')
        opening = prefix.index('{', position)
        depth, closing = 1, opening + 1
        while depth:
            depth += (prefix[closing] == '{') - (prefix[closing] == '}')
            closing += 1
        return textwrap.dedent(prefix[opening+1:closing-1]).strip()
    special = [('PACKET_EOF', 'package_eof'), ('WEBSOCKET_OPTIONS', None),
               ('MAX_IDLE', 'max_idle_time'), ('PACKET_TYPE', 'package_length_type'),
               ('LENGTH_OFFSET', 'package_length_offset'), ('BODY_OFFSET', 'package_body_offset'),
               ('LENGTH_CALLBACK', 'package_length_func')]
    statements = []
    for kind, key in special:
        native = effects(key) if key else 'php_swoole_server_set_websocket_option(port, vht);'
        statements.append('        case SWR_PORTS_SPECIAL_' + kind + ': {\n' + textwrap.indent(native, '            ') + '\n            break;\n        }')
    loop = '''    // backlog and following ordinary options use the sole immutable Rust catalog.
    for (size_t index = 0;; index++) {
        auto option = swr_v1_ports_option(index);
        if (option.action == SWR_PORTS_OPTION_END) break;
        bool present = true;
        if (option.policy != SWR_PORTS_OPTION_ALWAYS) {
            ztmp = zend_hash_str_find(vht, (const char *) option.key.data, option.key.len);
            present = ztmp && !ZVAL_IS_NULL(ztmp);
            if (!present && option.alias.len) {
                ztmp = zend_hash_str_find(vht, (const char *) option.alias.data, option.alias.len);
                present = ztmp && !ZVAL_IS_NULL(ztmp);
            }
            if (!present && option.policy == SWR_PORTS_MISSING_SKIP) continue;
        }
        if (option.action != SWR_PORTS_OPTION_SPECIAL) {
            int64_t value = 0;
            if (present) {
                switch (option.converter) {
                case SWR_PORTS_CONVERT_LONG: value = zval_get_long(ztmp); break;
                case SWR_PORTS_CONVERT_SIZE: value = php_swoole_parse_to_size(ztmp); break;
                case SWR_PORTS_CONVERT_TRUTH: value = zval_is_true(ztmp); break;
                default: assert(false);
                }
            }
            switch (option.action) {
            case SWR_PORTS_OPTION_NUMBER:
                port_apply_number(port, option.target, swr_v1_ports_number(option.number_kind, value));
                break;
            case SWR_PORTS_OPTION_BUFFER: {
                auto plan = swr_v1_ports_buffer_setting(value);
                assert(plan.count == 1 || plan.count == 2);
                port_apply_number(port, option.target, plan.first);
                if (plan.count == 2) port_apply_number(port, option.target, plan.last);
                break;
            }
            case SWR_PORTS_OPTION_FLAG:
                port_apply_flags(port, swr_v1_ports_flag_setting(option.target, value, present));
                break;
            default: assert(false);
            }
            continue;
        }
        switch (option.target) {
''' + '\n'.join(statements) + '''
        default: assert(false);
        }
    }

'''
    php = php[:start] + loop + php[end:]
    begin = php.index('template<class T> static void port_apply_buffer(')
    finish = php.index('\n}\n', begin) + 3
    php = php[:begin] + 'static void port_apply_number(ListenPort *port, uint32_t field, int64_t value) {\n    switch (field) {\n    case SWR_PORTS_NUMBER_BACKLOG: port->backlog = value; break;\n    case SWR_PORTS_NUMBER_SOCKET_BUFFER: port->socket_buffer_size = value; break;\n    case SWR_PORTS_NUMBER_KERNEL_RECV: port->kernel_socket_recv_buffer_size = value; break;\n    case SWR_PORTS_NUMBER_KERNEL_SEND: port->kernel_socket_send_buffer_size = value; break;\n    case SWR_PORTS_NUMBER_HEARTBEAT: port->heartbeat_idle_time = value; break;\n    case SWR_PORTS_NUMBER_HIGH_WATERMARK: port->buffer_high_watermark = value; break;\n    case SWR_PORTS_NUMBER_LOW_WATERMARK: port->buffer_low_watermark = value; break;\n    case SWR_PORTS_NUMBER_DEFER_ACCEPT: port->tcp_defer_accept = value; break;\n    case SWR_PORTS_NUMBER_KEEP_IDLE: port->tcp_keepidle = value; break;\n    case SWR_PORTS_NUMBER_KEEP_INTERVAL: port->tcp_keepinterval = value; break;\n    case SWR_PORTS_NUMBER_KEEP_COUNT: port->tcp_keepcount = value; break;\n    case SWR_PORTS_NUMBER_USER_TIMEOUT: port->tcp_user_timeout = value; break;\n    case SWR_PORTS_NUMBER_MAX_LENGTH: port->protocol.package_max_length = value; break;\n    default: assert(false);\n    }\n}\n' + php[finish:]
    return php


def tls_settings(php):
    php = body(php, 'static bool php_swoole_server_set_ssl_option(', '''    zval *ztmp;
    for (size_t index = 0;; index++) {
        auto option = swr_v1_ports_tls_option(SWR_PORTS_TLS_CONTEXT_FILES, index);
        if (option.action == SWR_PORTS_OPTION_END) break;
        ztmp = zend_hash_str_find(vht, (const char *) option.key.data, option.key.len);
        if (!ztmp || ZVAL_IS_NULL(ztmp)) continue;
        zend::String str_v(ztmp);
        switch (option.target) {
        case SWR_PORTS_TLS_CERT_FILE:
            if (!ctx->set_cert_file(str_v.to_std_string())) {
                php_swoole_fatal_error(E_ERROR, "ssl cert file[%s] not found", str_v.val());
                return false;
            }
            break;
        case SWR_PORTS_TLS_KEY_FILE:
            if (!ctx->set_key_file(str_v.to_std_string())) {
                php_swoole_fatal_error(E_ERROR, "ssl key file[%s] not found", str_v.val());
                return false;
            }
            break;
        default: assert(false);
        }
    }
    return true;''')
    method = php.index('static PHP_METHOD(swoole_server_port, set) {')
    start = php.index('        if (php_swoole_array_get_value(vht, "ssl_compress", ztmp)) {', method)
    end = php.index('        if (php_swoole_array_get_value(vht, "ssl_sni_certs", ztmp)) {', start)
    region = php[start:end]
    def effects(key):
        begin = region.index(f'if (php_swoole_array_get_value(vht, "{key}", ztmp)');opening = region.index('{', begin)
        depth, closing = 1, opening + 1
        while depth:
            depth += (region[closing] == '{') - (region[closing] == '}');closing += 1
        return textwrap.dedent(region[opening+1:closing-1]).strip()
    special = [('CLIENT_CERT', 'ssl_client_cert_file'), ('CAFILE', 'ssl_cafile'),
               ('CAPATH', 'ssl_capath'), ('CIPHERS', 'ssl_ciphers'),
               ('ECDH_CURVE', 'ssl_ecdh_curve'), ('DH_PARAM', 'ssl_dhparam')]
    statements = []
    for kind, key in special:
        statements.append('            case SWR_PORTS_TLS_' + kind + ': {\n' + textwrap.indent(effects(key), '                ') + '\n                break;\n            }')
    loop = '''        // TLS frontend order/conversion only; SNI and crypto algorithms remain native.
        for (size_t index = 0;; index++) {
            auto option = swr_v1_ports_tls_option(SWR_PORTS_TLS_PORT, index);
            if (option.action == SWR_PORTS_OPTION_END) break;
            ztmp = zend_hash_str_find(vht, (const char *) option.key.data, option.key.len);
            if (!ztmp || ZVAL_IS_NULL(ztmp)) continue;
            if (option.action != SWR_PORTS_OPTION_SPECIAL) {
                int64_t raw = option.converter == SWR_PORTS_CONVERT_TRUTH ? zval_is_true(ztmp) : zval_get_long(ztmp);
                auto value = swr_v1_ports_tls_value(option.target, raw);
                switch (option.target) {
                case SWR_PORTS_TLS_COMPRESS: port->set_ssl_disable_compress(value); break;
                case SWR_PORTS_TLS_PROTOCOLS: port->set_ssl_protocols(value); break;
                case SWR_PORTS_TLS_VERIFY_PEER: port->set_ssl_verify_peer(value); break;
                case SWR_PORTS_TLS_ALLOW_SELF_SIGNED: port->set_ssl_allow_self_signed(value); break;
                case SWR_PORTS_TLS_VERIFY_DEPTH: port->set_ssl_verify_depth(value); break;
                case SWR_PORTS_TLS_PREFER_CIPHERS: port->set_ssl_prefer_server_ciphers(value); break;
                default: assert(false);
                }
                continue;
            }
            switch (option.target) {
''' + '\n'.join(statements) + '''
            default: assert(false);
            }
        }
'''
    return php[:start] + loop + php[end:]


def callback_hooks(php):
    start = php.index('    if (index == SW_SERVER_CB_onConnect', php.index('static PHP_METHOD(swoole_server_port, on) {'))
    end = php.index('    RETURN_TRUE;', start)
    bindings = [('CONNECT', 'onConnect'), ('PACKET', 'onPacket'), ('CLOSE', 'onClose'),
                ('BUFFER_FULL', 'onBufferFull'), ('BUFFER_EMPTY', 'onBufferEmpty')]
    code = '    switch (swr_v1_ports_callback_hook(index)) {\n'
    for kind, name in bindings:
        code += f'    case SWR_PORTS_HOOK_{kind}:\n        if (!serv->{name}) serv->{name} = php_swoole_server_{name};\n        break;\n'
    code += '    default: break;\n    }\n'
    return php[:start] + code + php[end:]


def sni_admission(php):
    php = replace(php, '''        if (php_swoole_array_get_value(vht, "ssl_sni_certs", ztmp)) {
            if (Z_TYPE_P(ztmp) != IS_ARRAY) {''', '''        // SNI frontend admission only; native iteration/context/crypto remains authoritative.
        auto sni = swr_v1_ports_sni_plan(0, 0, 0);
        ztmp = zend_hash_str_find(vht, (const char *) sni.key.data, sni.key.len);
        if (ztmp && !ZVAL_IS_NULL(ztmp)) {
            auto mapping = swr_v1_ports_sni_plan(1, 0, Z_TYPE_P(ztmp) == IS_ARRAY);
            if (mapping.status == 1) {''')
    php = replace(php, '''                if (!key) {
                    php_swoole_fatal_error(E_WARNING, "ssl_sni_certs array requires string host name keys");''', '''                auto item = swr_v1_ports_sni_plan(2, key != nullptr, key ? Z_TYPE_P(current) == IS_ARRAY : false);
                if (item.status == 2) {
                    php_swoole_fatal_error(E_WARNING, "ssl_sni_certs array requires string host name keys");''')
    php = replace(php, 'if (Z_TYPE_P(current) != IS_ARRAY) {', 'if (item.status == 3) {')
    return php


def class_properties(php):
    start = php.index('    zend_declare_property_null(swoole_server_port_ce, ZEND_STRL("onConnect")')
    end = php.index('\n', php.index('    zend_declare_property_null(swoole_server_port_ce, ZEND_STRL("connections")', start))
    return php[:start] + '''    for (size_t i = 0;; i++) {
        auto property = swr_v1_ports_class_property(i);
        if (property.kind == SWR_PORTS_PROPERTY_END) break;
        auto flags = property.is_private ? ZEND_ACC_PRIVATE : ZEND_ACC_PUBLIC;
        const char *name = (const char *) property.name.data;
        switch (property.kind) {
        case SWR_PORTS_PROPERTY_NULL: zend_declare_property_null(swoole_server_port_ce, name, property.name.len, flags); break;
        case SWR_PORTS_PROPERTY_LONG: zend_declare_property_long(swoole_server_port_ce, name, property.name.len, property.value, flags); break;
        case SWR_PORTS_PROPERTY_BOOL: zend_declare_property_bool(swoole_server_port_ce, name, property.name.len, property.value, flags); break;
        default: assert(false);
        }
    }''' + php[end:]


def method(php, name):
    """The text of `static PHP_METHOD(swoole_server_port, NAME) {...}` and its span."""
    start = php.index(f'static PHP_METHOD(swoole_server_port, {name}) {{')
    opening = php.index('{', start)
    depth, end = 1, opening + 1
    while depth:
        depth += (php[end] == '{') - (php[end] == '}')
        end += 1
    return start, end


def rust_frontend(php):
    """crates/phpserver owns the class, its objects and every method body; C++ keeps
    the method table, ListenPort accessors Server uses, the length callback, and the
    ListenPort/TLS effects of set() as `port_apply_settings`, reached through a host
    table (native/ports/swoole_rs_phpserver.h)."""
    php = replace(php, '#include "swoole_rs_ports.h"', '#include "swoole_rs_ports.h"\n#include "swoole_rs_phpserver.h"')
    # set(): its ListenPort/TLS block becomes a C++ host function, upstream's text unchanged
    # except that its early exits return false (RETURN_FALSE) instead of writing return_value.
    start, end = method(php, 'set')
    text = php[start:end]
    block = text[text.index('    // backlog'):text.index('    if (SWOOLE_G(enable_library)) {')]
    block = block.replace('RETURN_FALSE;', 'return false;').replace('                    return;\n', '                    return false;\n')
    settings = ('static bool port_apply_settings(ListenPort *port, ServerPortProperty *property, HashTable *vht) {\n'
                '    zval *ztmp;\n' + block + '    return true;\n}\n')
    php = php[:start] + settings + php[end:]
    for name in ['__construct', '__destruct', 'on', 'getCallback']:
        start, end = method(php, name)
        php = php[:start] + php[end:].lstrip('\n')
    start, end = method(php, 'getSocket')
    php = php[:start] + php[end:].lstrip('\n')
    php = replace(php, '#ifdef SWOOLE_SOCKETS_SUPPORT\n#endif', '')
    # Methods are Rust's zim_* exports under Zend's own names.
    php = php.replace('static PHP_METHOD(swoole_server_port, ', 'PHP_METHOD(swoole_server_port, ')
    # Objects: Rust creates and frees them; C++ finds them by the struct's own layout.
    php = replace(php, 'static zend_object_handlers swoole_server_port_handlers;\n', '')
    php = replace(php, '(ServerPortObject *) ((char *) obj - swoole_server_port_handlers.offset)',
                  '(ServerPortObject *) ((char *) obj - offsetof(ServerPortObject, std))')
    start = php.index('static void php_swoole_server_port_free_object(')
    end = php.index('SW_EXTERN_C_BEGIN', start)
    php = php[:start] + php[end:]
    start = php.index('static ServerPortProperty *php_swoole_server_port_get_and_check_property(')
    end = php.index('// Dereference from server object', start)
    php = php[:start] + php[end:]
    start = php.index('void php_swoole_server_port_minit(int module_number) {')
    end = php.index('\n}\n', start) + 3
    minit = '''static bool port_apply_settings(ListenPort *port, ServerPortProperty *property, HashTable *vht);

namespace {
void port_invalid(zend_object *object) {
    php_swoole_fatal_error(E_ERROR, "Invalid instance of %s", ZSTR_VAL(object->ce->name));
}
bool port_started(void *server) {
    auto *serv = static_cast<Server *>(server);
    return !serv->is_worker_thread() && serv->is_started();
}
void port_warn(int kind, const char *name) {
    if (kind == 0) {
        php_swoole_fatal_error(E_WARNING, "can't register event callback function after server started");
    } else {
        php_swoole_error(E_WARNING, "unknown event types[%s]", name);
    }
}
void *port_callable_create(zval *callable) {
    return sw_callable_create(callable);
}
void port_callable_free(void *callable) {
    sw_callable_free(callable);
}
void port_hook(void *server, uint32_t hook) {
    auto *serv = static_cast<Server *>(server);
    switch (hook) {
    case SWR_PORTS_HOOK_CONNECT:
        if (!serv->onConnect) serv->onConnect = php_swoole_server_onConnect;
        break;
    case SWR_PORTS_HOOK_PACKET:
        if (!serv->onPacket) serv->onPacket = php_swoole_server_onPacket;
        break;
    case SWR_PORTS_HOOK_CLOSE:
        if (!serv->onClose) serv->onClose = php_swoole_server_onClose;
        break;
    case SWR_PORTS_HOOK_BUFFER_FULL:
        if (!serv->onBufferFull) serv->onBufferFull = php_swoole_server_onBufferFull;
        break;
    case SWR_PORTS_HOOK_BUFFER_EMPTY:
        if (!serv->onBufferEmpty) serv->onBufferEmpty = php_swoole_server_onBufferEmpty;
        break;
    default: break;
    }
}
bool port_settings(void *port, void *property, zend_array *vht) {
    return port_apply_settings(static_cast<ListenPort *>(port), static_cast<ServerPortProperty *>(property), vht);
}
bool port_finish(zend_object *object, zval *zset, void *property) {
    bool bailout = false;
    zend_try {
        if (SWOOLE_G(enable_library)) {
            zval params[1] = {
                *zset,
            };
            zend::function::call(R"(\\Swoole\\Server\\Helper::checkOptions)", 1, params);
        }
        zval zobject;
        ZVAL_OBJ(&zobject, object);
        zval *zsetting = sw_zend_read_and_convert_property_array(swoole_server_port_ce, &zobject, ZEND_STRL("setting"), 0);
        SEPARATE_ARRAY(zsetting);
        php_array_merge(Z_ARRVAL_P(zsetting), Z_ARRVAL_P(zset));
        static_cast<ServerPortProperty *>(property)->zsetting = zsetting;
    } zend_catch {
        bailout = true;
    } zend_end_try();
    return bailout;
}
void port_socket(void *port, zval *return_value) {
#ifdef SWOOLE_SOCKETS_SUPPORT
    php_socket *socket_object = php_swoole_convert_to_socket(static_cast<ListenPort *>(port)->get_fd());
    if (!socket_object) {
        RETURN_FALSE;
    }
    SW_ZVAL_SOCKET(return_value, socket_object);
    zval *zsocket = sw_zval_dup(return_value);
    Z_TRY_ADDREF_P(zsocket);
#else
    (void) port;
    RETURN_FALSE;
#endif
}
}  // namespace

void php_swoole_server_port_minit(int module_number) {
    static_assert(offsetof(ServerPortObject, property) == sizeof(void *) &&
                  offsetof(ServerPortObject, std) == 16 * sizeof(void *), "crates/phpserver port layout");
    static const swr_v1_phpserver_port_host host = {
        port_invalid, php_swoole_server_port_deref, port_started, port_warn, port_callable_create,
        port_callable_free, port_hook, port_settings, port_finish, port_socket,
    };
#ifndef SW_THREAD
    const bool not_serializable = true;
#else
    const bool not_serializable = false;
#endif
    swoole_server_port_ce = swr_v1_phpserver_port_minit(swoole_server_port_methods, &host, not_serializable);
}
'''
    php = php[:start] + minit + php[end:]
    return php


def generated():
    native = replace(original('src/server/port.cc'), '#include "swoole_server.h"', '#include "swoole_server.h"\n#include "swoole_rs_ports.h"')
    native = replace(native, 'namespace swoole {', '''namespace swoole {
static uint32_t port_protocol_flags(const ListenPort &p) {
    return (p.open_eof_check ? SWR_PORTS_EOF : 0) | (p.open_length_check ? SWR_PORTS_LENGTH : 0) |
           (p.open_http_protocol ? SWR_PORTS_HTTP : 0) | (p.open_http2_protocol ? SWR_PORTS_HTTP2 : 0) |
           (p.open_websocket_protocol ? SWR_PORTS_WEBSOCKET : 0) | (p.open_mqtt_protocol ? SWR_PORTS_MQTT : 0) |
           (p.open_redis_protocol ? SWR_PORTS_REDIS : 0);
}''')
    native = body(native, 'ListenPort::ListenPort(Server *server)', '''    static_assert(std::is_same<decltype(object_id), uint16_t>::value &&
                  std::is_same<decltype(protocol.package_max_length), uint32_t>::value, "ports constructor fields");
    static_assert(sizeof(SW_DATA_EOF) - 1 <= sizeof(protocol.package_eof) &&
                  sizeof(protocol.package_eof) <= UINT8_MAX, "ports constructor EOF");
    auto defaults = swr_v1_ports_defaults(SW_INPUT_BUFFER_SIZE, sizeof(SW_DATA_EOF) - 1);
    object_id = defaults.object_id;
    protocol.package_length_type = static_cast<char>(defaults.length_type);
    protocol.package_length_size = defaults.length_size;
    protocol.package_body_offset = defaults.body_offset;
    protocol.package_max_length = defaults.max_length;
    protocol.package_eof_len = defaults.eof_length;
    memcpy(protocol.package_eof, SW_DATA_EOF, protocol.package_eof_len);
    protocol.private_data_2 = server;''')
    native = body(native, 'size_t ListenPort::get_connection_num() const', '''    static_assert(sizeof(size_t) == sizeof(uint64_t), "ports count ABI");
    static_assert(std::is_same<sw_atomic_t, volatile uint32_t>::value, "ports native counter ABI");
    auto *counters = gs->connection_nums;
    return swr_v1_ports_connection_count(counters, counters ? sw_server()->worker_num : 0,
                                       counters ? 0 : gs->connection_num);''')
    native = body(native, 'void ListenPort::set_eof_protocol(const std::string &eof, bool find_from_right)', '''    static_assert(sizeof(protocol.package_eof) <= UINT8_MAX, "ports EOF length field");
    auto settings = swr_v1_ports_framing_settings({SWR_PORTS_EOF_SETTINGS, find_from_right,
        eof.length(), sizeof(protocol.package_eof), 0, 0, 0, 0});
    open_eof_check = settings.enabled;
    protocol.split_by_eof = settings.split;
    protocol.package_eof_len = settings.length;
    memcpy(protocol.package_eof, eof.c_str(), protocol.package_eof_len);''')
    native = body(native, 'void ListenPort::set_length_protocol(uint32_t length_offset, char length_type, uint32_t body_offset)', '''    static_assert(std::is_same<decltype(protocol.package_length_size), uint8_t>::value &&
                  std::is_same<decltype(protocol.package_length_offset), uint16_t>::value &&
                  std::is_same<decltype(protocol.package_body_offset), uint16_t>::value, "ports length fields");
    auto settings = swr_v1_ports_framing_settings({SWR_PORTS_LENGTH_SETTINGS, 0, 0, 0,
        static_cast<uint8_t>(length_type), swoole_type_size(length_type), length_offset, body_offset});
    open_length_check = settings.enabled;
    protocol.package_length_type = static_cast<char>(settings.length_type);
    protocol.package_length_size = settings.length_size;
    protocol.package_length_offset = settings.length_offset;
    protocol.package_body_offset = settings.body_offset;''')
    native = body(native, 'void ListenPort::set_stream_protocol()', '''    auto settings = swr_v1_ports_stream_settings();
    open_length_check = settings.enabled;
    protocol.get_package_length = Protocol::default_length_func;
    protocol.package_length_type = static_cast<char>(settings.length_type);
    protocol.package_length_size = settings.length_size;
    protocol.package_body_offset = settings.body_offset;
    protocol.package_length_offset = settings.length_offset;''')
    native = body(native, 'int ListenPort::listen()', r'''    constexpr uint32_t capabilities = 0
#ifdef TCP_DEFER_ACCEPT
        | 1
#endif
#ifdef SO_ACCEPTFILTER
        | 2
#endif
#ifdef TCP_FASTOPEN
        | 4
#endif
#ifdef SO_KEEPALIVE
        | 8
#endif
#ifdef TCP_KEEPIDLE
        | 16
#endif
#ifdef TCP_USER_TIMEOUT
        | 32
#endif
        ;
    SwrPortsListenState state{};
    int result = 0;
    while (true) {
        SwrPortsListenInput input{capabilities, listening, open_tcp_keepalive, buffer_high_watermark,
            tcp_defer_accept, tcp_fastopen, tcp_keepidle, tcp_keepinterval, tcp_keepcount, tcp_user_timeout, result};
        auto action = swr_v1_ports_listen_step(&state, &input);
#define PORTS_OPTION(KIND, LEVEL, OPTION, CMP) \
        case KIND: \
            if (socket->set_option(LEVEL, OPTION, action.value) CMP 0) { \
                swoole_sys_warning("setsockopt(" #OPTION ") failed"); \
            } \
            break
        switch (action.kind) {
        case 0: return action.status;
        case 1:
            result = socket->listen(backlog);
            if (result < 0) { swoole_sys_warning("listen(%s:%d, %d) failed", host.c_str(), port, backlog); }
            break;
        case 2: listening = true; break;
#ifdef TCP_DEFER_ACCEPT
        PORTS_OPTION(3, IPPROTO_TCP, TCP_DEFER_ACCEPT, !=);
#endif
#ifdef SO_ACCEPTFILTER
        case 4: {
            struct accept_filter_arg a;
            memset(&a, 0, sizeof(a));
            strcpy(a.af_name, "httpready");
            if (socket->set_option(SOL_SOCKET, SO_ACCEPTFILTER, &a, sizeof(a)) != 0) {
                swoole_sys_warning("setsockopt(SO_ACCEPTFILTER) failed");
            }
            break;
        }
#endif
#ifdef TCP_FASTOPEN
        PORTS_OPTION(5, IPPROTO_TCP, TCP_FASTOPEN, !=);
#endif
#ifdef SO_KEEPALIVE
        PORTS_OPTION(6, SOL_SOCKET, SO_KEEPALIVE, !=);
#ifdef TCP_KEEPIDLE
        PORTS_OPTION(7, IPPROTO_TCP, TCP_KEEPIDLE, <);
        PORTS_OPTION(8, IPPROTO_TCP, TCP_KEEPINTVL, <);
        PORTS_OPTION(9, IPPROTO_TCP, TCP_KEEPCNT, <);
#endif
#ifdef TCP_USER_TIMEOUT
        PORTS_OPTION(10, IPPROTO_TCP, TCP_USER_TIMEOUT, !=);
#endif
#endif
        case 11: buffer_high_watermark = socket_buffer_size * 0.8; break;
        default: return SW_ERR;
        }
#undef PORTS_OPTION
    }''')
    native = body(native, 'void ListenPort::init_protocol()', '''    const bool datagram = is_dgram();
    const uint32_t flags = port_protocol_flags(*this) | (datagram ? SWR_PORTS_DGRAM : 0) |
                           (datagram && is_dtls() ? SWR_PORTS_DTLS : 0);
    const auto plan = swr_v1_ports_protocol_plan({flags, (uint8_t) protocol.package_length_type,
        protocol.package_eof_len, SW_DATA_EOF_MAXLEN, SW_HTTP2_FRAME_HEADER_SIZE,
        SW_WEBSOCKET_FRAME_HEADER_SIZE, SW_MQTT_MAX_LENGTH_SIZE});
    using Reader = int (*)(Reactor *, ListenPort *, Event *);
    static constexpr Reader readers[] = {nullptr, readable_callback_eof, readable_callback_length,
        readable_callback_http, readable_callback_redis, readable_callback_raw};
    using Package = int (*)(const Protocol *, Socket *, const RecvData *);
    static constexpr Package packages[] = {nullptr, Server::dispatch_task,
        http_server::dispatch_frame, websocket::dispatch_frame};
    static constexpr Protocol::LengthFunc lengths[] = {nullptr, Protocol::default_length_func,
        http_server::get_package_length, http2::get_frame_length, websocket::get_package_length, mqtt::get_package_length};
    for (uint32_t i = 0; i < plan.count; i++) {
        const auto action = plan.actions[i];
        switch (action.kind) {
        case SWR_PORTS_SET_EOF_LEN: protocol.package_eof_len = action.value; break;
        case SWR_PORTS_SET_READ: onRead = readers[action.value]; break;
        case SWR_PORTS_SET_PACKAGE: protocol.onPackage = packages[action.value]; break;
        case SWR_PORTS_SET_LENGTH_FN: protocol.get_package_length = lengths[action.value]; break;
        case SWR_PORTS_SET_LENGTH_SIZE_FN: protocol.get_package_length_size = http_server::get_package_length_size; break;
        case SWR_PORTS_SET_LENGTH_SIZE: protocol.package_length_size = action.value; break;
        case SWR_PORTS_SET_LENGTH_OFFSET: protocol.package_length_offset = action.value; break;
        case SWR_PORTS_SET_BODY_OFFSET: protocol.package_body_offset = action.value; break;
        }
    }''')
    native = body(native, 'const char *ListenPort::get_protocols() const', '''    return swr_v1_ports_protocol_name(port_protocol_flags(*this) | (is_dgram() ? SWR_PORTS_DGRAM : 0));''')
    native = replace(native, '#include "swoole_rs_ports.h"', '#include "swoole_rs_ports.h"\n#include <type_traits>')
    native = body(native, 'int ListenPort::create_socket()', '''    auto *server = static_cast<Server *>(protocol.private_data_2);
    // Address has no construction/destruction effects; storage may precede the decisions.
    static_assert(std::is_trivial<Address>::value, "ports Address lifetime boundary");
    Address addr;
    constexpr uint32_t capabilities = 0
#if defined(__linux__) && defined(HAVE_REUSEPORT)
        | 1
#endif
#if defined(SW_SUPPORT_DTLS) && defined(HAVE_KQUEUE)
        | 2
#endif
        ;
    SwrPortsSocketState state{};
    int result = 0;
    while (true) {
        SwrPortsSocketInput input{capabilities, socket != nullptr, result};
        auto action = swr_v1_ports_socket_step(&state, &input);
        switch (action.kind) {
        case 0: return action.status;
#if defined(__linux__) && defined(HAVE_REUSEPORT)
        case 1: result = server->enable_reuse_port; break;
#endif
        case 2: close_socket(); break;
        case 3:
            socket = make_socket(type, is_dgram() ? SW_FD_DGRAM_SERVER : SW_FD_STREAM_SERVER,
                                 SW_SOCK_CLOEXEC | SW_SOCK_NONBLOCK);
            break;
        case 4: swoole_set_last_error(errno); return SW_ERR;
#if defined(SW_SUPPORT_DTLS) && defined(HAVE_KQUEUE)
        case 5: result = is_dtls(); break;
        case 6: socket->set_reuse_port(); break;
#endif
#if defined(__linux__) && defined(HAVE_REUSEPORT)
        case 7: result = socket->set_reuse_port(); break;
#endif
        case 8: swoole_set_last_error(errno); socket->free(); return SW_ERR;
        case 9: result = addr.assign(type, host, port, true); break;
        case 10: {
            auto type_str = Address::type_str(type);
            swoole_warning("Invalid %s address '%s:%d'", type_str, host.c_str(), port);
            break;
        }
        case 11:
            if (socket->set_reuse_addr() < 0) { swoole_sys_warning("setsockopt(%d, SO_REUSEADDR) failed", socket->get_fd()); }
            break;
        case 12: result = socket->bind(addr); break;
        case 13: result = socket->get_name(); break;
        case 14: port = socket->get_port(); break;
        default: return SW_ERR;
        }
    }''')

    php = replace(original('ext-src/swoole_server_port.cc'), '#include "php_swoole_server.h"', '#include "php_swoole_server.h"\n#include "swoole_rs_ports.h"')
    start = php.index('struct ServerPortEvent {')
    end = php.index('// clang-format on', start) + len('// clang-format on')
    php = php[:start] + '''static_assert(sizeof(zend_long) == 8, "ports PHP integer ABI");
static_assert(SW_SERVER_CB_onConnect == 0 && SW_SERVER_CB_onReceive == 1 && SW_SERVER_CB_onClose == 2 &&
              SW_SERVER_CB_onPacket == 3 && SW_SERVER_CB_onRequest == 4 && SW_SERVER_CB_onHandshake == 5 &&
              SW_SERVER_CB_onBeforeHandshakeResponse == 6 && SW_SERVER_CB_onOpen == 7 &&
              SW_SERVER_CB_onMessage == 8 && SW_SERVER_CB_onDisconnect == 9 &&
              SW_SERVER_CB_onBufferFull == 10 && SW_SERVER_CB_onBufferEmpty == 11, "ports callback ABI");''' + php[end:]
    php = php.replace('SW_MAX(0, SW_MIN(v, UINT16_MAX))', 'swr_v1_ports_number(SWR_PORTS_U16, v)')
    php = php.replace('SW_MAX(0, SW_MIN(v, UINT32_MAX))', 'swr_v1_ports_number(SWR_PORTS_U32, v)')
    php = php.replace('SW_MAX(0, SW_MIN(v, UINT8_MAX))', 'swr_v1_ports_number(SWR_PORTS_U8, v)')
    php = php.replace('SW_MAX(INT_MIN, SW_MIN(v, INT_MAX))', 'swr_v1_ports_number(SWR_PORTS_I32, v)')
    for field in ['socket_buffer_size', 'kernel_socket_recv_buffer_size', 'kernel_socket_send_buffer_size']:
        php = replace(php, f'''port->{field} = swr_v1_ports_number(SWR_PORTS_I32, v);
        if (port->{field} <= 0) {{
            port->{field} = INT_MAX;
        }}''', f'port_apply_buffer(port->{field}, swr_v1_ports_buffer_setting(v));')
    php = replace(php, '''        port->protocol.package_eof_len = str_v.len();
        if (port->protocol.package_eof_len == 0) {''', '''        static_assert(sizeof(port->protocol.package_eof) == SW_DATA_EOF_MAXLEN && SW_DATA_EOF_MAXLEN <= UINT8_MAX, "ports EOF capacity");
        auto packet = swr_v1_ports_packet_eof(str_v.len(), sizeof(port->protocol.package_eof));
        port->protocol.package_eof_len = packet.value;
        if (packet.status == 1) {''')
    php = replace(php, 'else if (port->protocol.package_eof_len > SW_DATA_EOF_MAXLEN)', 'else if (packet.status == 2)')
    php = replace(php, 'memcpy(port->protocol.package_eof, str_v.val(), str_v.len());', 'memcpy(port->protocol.package_eof, str_v.val(), packet.copy_len);')
    php = replace(php, '''        port->protocol.package_length_size = swoole_type_size(port->protocol.package_length_type);
        if (port->protocol.package_length_size == 0) {''', '''        auto packet = swr_v1_ports_packet_width(swoole_type_size(port->protocol.package_length_type));
        port->protocol.package_length_size = packet.value;
        if (packet.status) {''')
    for field in ['package_length_offset', 'package_body_offset']:
        php = replace(php, f'''        port->protocol.{field} = swr_v1_ports_number(SWR_PORTS_U16, v);
        if (port->protocol.{field} > SW_IPC_BUFFER_SIZE) {{''', f'''        static_assert(SW_IPC_BUFFER_SIZE <= UINT32_MAX, "ports offset bound");
        auto packet = swr_v1_ports_packet_offset(v, SW_IPC_BUFFER_SIZE);
        port->protocol.{field} = packet.value;
        if (packet.status) {{''')
    flag_fields = [
        ('open_tcp_nodelay', 'NODELAY'), ('open_tcp_keepalive', 'KEEPALIVE'),
        ('open_eof_check', 'CHECK_EOF'), ('protocol.split_by_eof', 'SPLIT_EOF'),
        ('open_http_protocol', 'HTTP_FLAG'), ('open_websocket_protocol', 'WEBSOCKET_FLAG'),
        ('open_http2_protocol', 'HTTP2_FLAG'), ('open_mqtt_protocol', 'MQTT_FLAG'),
        ('open_redis_protocol', 'REDIS_FLAG'), ('tcp_fastopen', 'FASTOPEN'),
        ('open_length_check', 'CHECK_LENGTH')]
    helper = 'template<class T> static void port_apply_buffer(T &field, SwrPortsBufferSetting plan) {\n    assert(plan.count == 1 || plan.count == 2);\n    field = plan.first;\n    if (plan.count == 2) field = plan.last;\n}\n'
    helper += 'static void port_apply_flags(ListenPort *port, SwrPortsProtocolPlan plan) {\n    assert(plan.count <= 2);\n    for (uint32_t n = 0; n < plan.count; n++) {\n        switch (plan.actions[n].kind) {\n'
    for field, kind in flag_fields:
        helper += f'        case SWR_PORTS_SET_{kind}: port->{field} = plan.actions[n].value; break;\n'
    helper += '        default: assert(false);\n        }\n    }\n}\n'
    php = replace(php, 'static_assert(sizeof(zend_long) == 8,', helper + 'static_assert(sizeof(zend_long) == 8,')
    php = replace(php, '''    if (php_swoole_array_get_value(vht, "open_tcp_nodelay", ztmp)) {
        port->open_tcp_nodelay = zval_is_true(ztmp);
    } else {
        port->open_tcp_nodelay = true;
    }''', '''    bool has_nodelay = php_swoole_array_get_value(vht, "open_tcp_nodelay", ztmp);
    auto nodelay = has_nodelay ? zval_is_true(ztmp) : false;
    port_apply_flags(port, swr_v1_ports_flag_setting(SWR_PORTS_SET_NODELAY, nodelay, has_nodelay));''')
    for field, kind in flag_fields[1:]:
        getter = 'zval_get_long' if kind == 'REDIS_FLAG' else 'zval_is_true'
        old = f'        port->{field} = {getter}(ztmp);'
        dependent = {'SPLIT_EOF': 'open_eof_check', 'WEBSOCKET_FLAG': 'open_http_protocol', 'HTTP2_FLAG': 'open_http_protocol'}.get(kind)
        if dependent:
            old += f'\n        if (port->{field}) {{\n            port->{dependent} = true;\n        }}'
        php = replace(php, old, f'        port_apply_flags(port, swr_v1_ports_flag_setting(SWR_PORTS_SET_{kind}, {getter}(ztmp), 1));')
    start = php.index('    bool found = false;', php.index('static PHP_METHOD(swoole_server_port, on) {'))
    end = php.index('    RETURN_TRUE;', start)
    retained = php[php.index('        zend_update_property(', start):php.index('        break;', start)]
    retained = '\n'.join(line[4:] if line.startswith('    ') else line for line in retained.splitlines())
    retained = replace(retained, 'property_name.c_str(), property_name.length()', '(const char *) event.property.data, event.property.len')
    php = php[:start] + '''    auto event = swr_v1_ports_event((const uint8_t *) name, len, 1);
    if (event.index < 0) {
        php_swoole_error(E_WARNING, "unknown event types[%s]", name);
        RETURN_FALSE;
    }
    int index = event.index;
''' + retained + '\n' + php[end:]
    php = replace(php, '''    zend::String _event_name_tolower(zend_string_tolower(_event_name_ori.get()), false);
    auto i = server_port_event_map.find(_event_name_tolower.to_std_string());
    if (i != server_port_event_map.end()) {
        std::string property_name = "on" + i->second.name;''', '''    auto property_name = swr_v1_ports_callback_property((const uint8_t *) _event_name_ori.val(), _event_name_ori.len());
    if (property_name.data) {''')
    php = replace(php, 'property_name.c_str(),\n                                                property_name.length(),', '(const char *) property_name.data,\n                                                property_name.len,')
    php = ordinary_settings(php)
    php = tls_settings(php)
    php = sni_admission(php)
    php = callback_hooks(php)
    php = class_properties(php)
    php = rust_frontend(php)
    return {'src/server/port.cc': native, 'ext-src/swoole_server_port.cc': php}


def main():
    """Rewrite 0002 (swoole_server_port.cc). 0001 (port.cc) has been edited with
    tools/patches.py since (#764, #780), so its generator here is stale and it is kept."""
    directory = ROOT / 'migration/patches/ports'
    path = 'ext-src/swoole_server_port.cc'
    text = generated()[path]
    raw = (f'diff --git a/{path} b/{path}\n' + ''.join(difflib.unified_diff(original(path).splitlines(True), text.splitlines(True), fromfile='a/'+path, tofile='b/'+path, n=1))).replace('\n \n', '\n\n').encode()
    entry = next(patch for patch in module_patches(directory) if patch['upstream_paths'] == [path])
    (directory / entry['path']).write_bytes(raw)


if __name__ == '__main__':
    main()
