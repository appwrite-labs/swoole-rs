#include "swoole_rs_protocols.h"
#include <algorithm>
#include <array>
#include <cassert>
#include <cerrno>
#include <cinttypes>
#include <climits>
#include <cstdarg>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <functional>
#include <string>
#include <vector>
#include <memory>
#include <arpa/inet.h>
#include <sys/uio.h>
#include "swoole_proxy.h"
static void swoole_set_last_error(int) {}
#define swoole_error_log(...) ((void)0)
#define swoole_warning(...) ((void)0)
#define swoole_sys_warning(...) ((void)0)
#define swoole_trace_log(...) ((void)0)
#define swoole_trace(...) ((void)0)
#define SW_CRLF "\r\n"
#define SW_CRLF_LEN 2
#define SW_STRL(value) value, sizeof(value)-1
#define SW_MQTT_MIN_LENGTH_SIZE 1
#define SW_MQTT_MAX_LENGTH_SIZE 4
#define SW_MQTT_RECV_LEN_AGAIN 0
#define sw_malloc std::malloc
#define sw_memset_zero(pointer,length) std::memset(pointer,0,length)
namespace reference {
#include "adapter.inc"
// GCC does not recognize upstream's "no break" comments as annotations.
// Keep the oracle bodies unchanged and scope this diagnostic to them alone.
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wimplicit-fallthrough"
#include "reference.inc"
#pragma GCC diagnostic pop
}
// Rust's export under upstream's symbol (crates/protocols-ffi/src/mqtt.rs).
namespace swoole {
struct Protocol;
struct PacketLength;
namespace network {
struct Socket;
}
namespace mqtt {
ssize_t get_package_length(const Protocol *protocol, network::Socket *socket, PacketLength *pl);
}
}  // namespace swoole
namespace candidate {
using ::swoole::RustStringView;
#include "adapter.inc"
#include "candidate.inc"
namespace mqtt {
ssize_t get_package_length(const Protocol *, network::Socket *, PacketLength *pl) {
    return ::swoole::mqtt::get_package_length(nullptr, nullptr, reinterpret_cast<::swoole::PacketLength *>(pl));
}
}
namespace redis {
#include "redis_reply.inc"
// Rust's receive loop (crates/protocols-ffi/src/resp_recv.rs) over this model's socket,
// with the effects production's export performs on the real one.
int recv_packet(Protocol *protocol, Connection *conn, String *buffer) {
    struct Io { Protocol *protocol; Connection *conn; String *buffer; } io{protocol, conn, buffer};
    const swr_resp_io effects{&io,
        [](void *context, uint8_t *at, size_t n) -> ssize_t {
            auto *c = static_cast<Io *>(context)->conn;
            const ssize_t received = c->socket->recv(reinterpret_cast<char *>(at), n, 0);
            if (received >= 0) return received;
            return c->socket->catch_read_error(errno) == SW_CLOSE ? -1 : -2;
        },
        [](void *context, size_t size) { static_cast<Io *>(context)->buffer->extend(size); },
        [](void *context, size_t length) -> int {
            auto *x = static_cast<Io *>(context);
            RecvData data{};
            data.info.len = length;
            data.data = x->buffer->str;
            if (x->protocol->onPackage(x->protocol, x->conn->socket, &data) < 0) return -1;
            return x->conn->socket->removed ? 1 : 0;
        }};
    return swr_v1_resp_recv(&effects, buffer, reinterpret_cast<swr_resp_request **>(&conn->object),
                            protocol->package_max_length, swoole_pagesize());
}
}
}
// Execute the exact original PHP parser loop with only Zend allocation modeled.
// Actual Zend callbacks, yielding and allocation are covered by request.py.
namespace php_reference {
using zval = std::vector<std::pair<uint32_t, std::string>>;
static void array_init(zval *) {}
static void add_next_index_null(zval *out) { out->emplace_back(2, ""); }
static void add_next_index_long(zval *out, int value) { out->emplace_back(1, std::to_string(value)); }
static void add_next_index_stringl(zval *out, const char *data, size_t length) { out->emplace_back(0, std::string(data,length)); }
struct Request {
    std::string command;
    bool seen;
    zval params;
    bool operator==(const Request &other) const { return command==other.command && seen==other.seen && params==other.params; }
};
static Request parse(const std::string &wire) {
    namespace Redis=reference::redis;
#define Z_STRVAL(unused) wire.data()
#define Z_STRLEN(unused) wire.size()
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wimplicit-fallthrough"
#include "php-request-original.inc"
#pragma GCC diagnostic pop
#undef Z_STRVAL
#undef Z_STRLEN
    return {command ? std::string(command,command_len) : "", bool(add_param), std::move(zparams)};
}
}
static php_reference::Request rust_php_request(const std::string &wire) {
    php_reference::Request output{};
    swr_resp_php_state state{};
    swr_resp_span event{};
    uint8_t range=0;
    int32_t result;
    while((result=swr_v1_resp_php_next(reinterpret_cast<const uint8_t *>(wire.data()),wire.size(),&state,&event,&range))>0) {
        if(range)errno=ERANGE;
        if(event.kind==3) {output.command.assign(wire.data()+event.offset,event.length);output.seen=true;}
        else if(event.kind==0)output.params.emplace_back(0,std::string(wire.data()+event.offset,event.length));
        else if(event.kind==1)output.params.emplace_back(1,std::to_string(event.integer));
        else {assert(event.kind==2);output.params.emplace_back(2,"");}
    }
    if(range)errno=ERANGE;
    assert(result==0);
    return output;
}
// C printf truncation plus ASCII case model. Actual Zend case folding/property
// behavior is compared separately by handlers.py on real PHP engines.
static size_t handler_key_trace() {
    size_t cases=0;
    const auto check=[&](const std::string &wire) {
        std::array<char,64> expected{};
        const auto needed=std::snprintf(expected.data(),expected.size(),"_handler_%s",wire.c_str());
        assert(needed>=0);
        const auto length=std::min(size_t(needed),expected.size()-1);
        for(size_t i=0;i<length;++i)if(expected[i]>='A'&&expected[i]<='Z')expected[i]+='a'-'A';
        std::array<uint8_t,66> actual;actual.fill(99);
        assert(swr_v1_resp_handler_key(reinterpret_cast<const uint8_t *>(wire.data()),wire.size(),actual.data()+1)==length);
        assert(std::memcmp(expected.data(),actual.data()+1,length+1)==0);
        assert(actual.front()==99&&actual.back()==99);
        ++cases;
    };
    for(unsigned byte=0;byte<256;++byte)for(size_t length:{0u,1u,53u,54u,55u,63u,64u,65u,1024u})check(std::string(length,char(byte)));
    for(size_t offset=0;offset<128;++offset) {std::string wire(128,'A');wire[offset]=0;check(wire);}
    return cases;
}
// Generated grammar keeps every native bulk copy in bounds; malformed numeric
// delimiters/negative non-null bulk sizes are deliberately excluded because the
// unbounded original oracle can dereference nullptr/read outside input.
static size_t seeded_resp_trace(uint64_t *corpus) {
    uint64_t fingerprint=UINT64_C(14695981039346656037);
    uint64_t random = UINT64_C(0x7377721342632026);
    auto next = [&]() { random ^= random << 13; random ^= random >> 7; random ^= random << 17; return random; };
    const std::vector<std::string> whitespace{"", " ", "\t", "\n", "\v", "\f", "\r", "\r\n", " \t\v\f\r\n"};
    const std::vector<std::string> numbers{"0", "+0", "-0", "1", "+17", "00012", "2147483647", "2147483648",
        "4294967295", "9223372036854775807", "9223372036854775808", "-9223372036854775808",
        "-9223372036854775809", std::string(128,'9'), "-"+std::string(128,'9')};
    auto numeric = [&]() {
        const size_t spacing=next()%whitespace.size();
        const size_t digits=next()%numbers.size();
        return whitespace[spacing]+numbers[digits];
    };
    for (size_t trial=0;trial<10000;++trial) {
        const uint64_t seed=random;
        std::string wire="*"+numeric()+"\r\n";
        const size_t arguments=next()%33;
        for (size_t argument=0;argument<arguments;++argument) {
            const auto kind=next()%4;
            if (kind==0) wire+="$"+whitespace[next()%whitespace.size()]+"-1\r\n";
            else if (kind==1) wire+=":"+numeric()+"\r\n";
            else {
                const size_t length=next()%65;
                auto digits=std::to_string(length);
                const auto style=next()%4;
                if(style==0)digits="+"+digits;else if(style==1)digits="000"+digits;else if(style==2&&length==0)digits="-000";
                wire+="$"+whitespace[next()%whitespace.size()]+digits+"\r\n";
                for(size_t i=0;i<length;++i)wire+=char(next()&255);
                // Legacy parse skips these bytes without validating CRLF.
                wire+=char(next()&255);wire+=char(next()&255);
            }
        }
        for(unsigned shift=0;shift<64;shift+=8) {
            fingerprint^=(uint64_t(wire.size())>>shift)&255;fingerprint*=UINT64_C(1099511628211);
        }
        for(unsigned char byte:wire) {fingerprint^=byte;fingerprint*=UINT64_C(1099511628211);}
        errno=EDOM;const auto expected=reference::redis::parse(wire.data(),wire.size());const int expected_errno=errno;
        errno=EDOM;const auto actual=candidate::redis::parse(wire.data(),wire.size());const int actual_errno=errno;
        if(expected!=actual||expected_errno!=actual_errno) {
            std::fprintf(stderr,"defined RESP mismatch trial=%zu seed=%" PRIx64 "\n",trial,seed);
            assert(expected==actual&&expected_errno==actual_errno);
        }
        errno=EDOM;const auto php_expected=php_reference::parse(wire);const int php_expected_errno=errno;
        errno=EDOM;const auto php_actual=rust_php_request(wire);const int php_actual_errno=errno;
        if(!(php_expected==php_actual)||php_expected_errno!=php_actual_errno) {
            std::fprintf(stderr,"PHP request mismatch trial=%zu seed=%" PRIx64 "\n",trial,seed);
            assert(php_expected==php_actual&&php_expected_errno==php_actual_errno);
        }
    }
    *corpus=fingerprint;
    return 10000;
}
int main() {
    static_assert(sizeof(swr_resp_span)==24 && sizeof(swr_resp_request)==24);
    static_assert(sizeof(swr_socks_transition)==4);
    size_t mqtt = 0, socks = 0, resp = 0, receives = 0;
    for (uint32_t value : {0u,1u,127u,128u,16383u,16384u,2097151u,2097152u,268435455u}) {
        std::vector<char> packet{char(0x30)};
        auto remaining = value;
        do { uint8_t byte = remaining%128; remaining /= 128; if (remaining) byte |=128; packet.push_back(char(byte)); } while (remaining);
        for (size_t size=1;size<=packet.size();++size) {
            reference::PacketLength original{packet.data(),uint32_t(size),99};
            candidate::PacketLength actual{packet.data(),uint32_t(size),99};
            assert(reference::mqtt::get_package_length(nullptr,nullptr,&original)==candidate::mqtt::get_package_length(nullptr,nullptr,&actual));
            assert(actual.header_len==99); ++mqtt;
        }
    }
    for (unsigned first=0;first<256;++first) for(unsigned second=0;second<256;++second) {
        std::array<char,5> packet{0,char(first),char(second),char(128),char(128)};
        reference::PacketLength original{packet.data(),5,99}; candidate::PacketLength actual{packet.data(),5,99};
        assert(reference::mqtt::get_package_length(nullptr,nullptr,&original)==candidate::mqtt::get_package_length(nullptr,nullptr,&actual)); ++mqtt;
    }
    for (unsigned state : {0u,1u,2u,3u,4u,255u}) for (unsigned method : {0u,2u,255u}) {
        for(unsigned version=0;version<256;++version) for(unsigned reply : {0u,1u,2u,8u,255u}) {
            for(int sent : {-1,0,1}) {
                reference::Socks5Proxy expected{};candidate::Socks5Proxy actual{};
                expected.state=actual.state=state;expected.method=actual.method=method;
                expected.dns_tunnel=actual.dns_tunnel=1;expected.host=actual.host="proxy";
                expected.target_host=actual.target_host="destination.local";expected.target_port=actual.target_port=65535;
                expected.username=actual.username="user";expected.password=actual.password="pass";
                std::string expected_wire,actual_wire;
                const std::array<char,2> response{char(version),char(reply)};
                const bool before=expected.handshake(response.data(),2,[&](const char *data,size_t size){ expected_wire.assign(data,size);return sent==1?ssize_t(size):sent; });
                const bool after=actual.handshake(response.data(),2,[&](const char *data,size_t size){ actual_wire.assign(data,size);return sent==1?ssize_t(size):sent; });
                assert(before==after && expected.state==actual.state && expected_wire==actual_wire); ++socks;
            }
        }
    }
    for(size_t length : {size_t(0),size_t(1),size_t(250),size_t(255),size_t(256),size_t(480),size_t(505)}) {
        reference::Socks5Proxy original{};candidate::Socks5Proxy actual{};
        original.dns_tunnel=actual.dns_tunnel=1;original.target_host=actual.target_host=std::string(length,'d');
        original.target_port=actual.target_port=-1;
        auto before=original.pack_connect_request(),after=actual.pack_connect_request();
        assert(before==after && std::memcmp(original.buf,actual.buf,before<0?sizeof(original.buf):size_t(before))==0);++socks; // Failure: prefix and untouched tail.
        if(length<=250) {
            original.username=actual.username=std::string(length,'u');original.password=actual.password=std::string(length,'p');
            before=original.pack_auth_request();after=actual.pack_auth_request();assert(before==after && std::memcmp(original.buf,actual.buf,before)==0);++socks;
            before=original.pack_negotiate_request();after=actual.pack_negotiate_request();assert(before==after&&original.method==actual.method&&std::memcmp(original.buf,actual.buf,before)==0);++socks;
        }
    }
    for (int code = -32; code <= 288; ++code) {
        assert(std::strcmp(reference::Socks5Proxy::strerror(code), candidate::Socks5Proxy::strerror(code)) == 0);
        ++socks;
    }
    for (int code : {INT_MIN, INT_MAX}) {
        assert(std::strcmp(reference::Socks5Proxy::strerror(code), candidate::Socks5Proxy::strerror(code)) == 0);
        ++socks;
    }
    for (size_t user_length : {0u,1u,249u,250u,251u,255u,256u})
        for (size_t password_length : {0u,1u,249u,250u,251u,255u,256u})
            for (size_t host_length : {0u,480u,481u}) for (int socket_type : {-1,1,2}) {
                std::string user(user_length, 'u'), password(password_length, 'p'), host(host_length, 'h');
                if (user_length > 1) user[1] = 0;
                if (password_length > 1) password[1] = 0;
                std::unique_ptr<reference::Socks5Proxy> before(reference::Socks5Proxy::create(socket_type, host, -1, user, password));
                std::unique_ptr<candidate::Socks5Proxy> after(candidate::Socks5Proxy::create(socket_type, host, -1, user, password));
                assert(bool(before) == bool(after));
                if (before) {
                    assert(before->host == after->host && before->port == after->port && before->socket_type == after->socket_type);
                    assert(before->dns_tunnel == after->dns_tunnel && before->username == after->username && before->password == after->password);
                    assert(before->state == after->state && before->version == after->version && before->method == after->method);
                    auto x=before->pack_negotiate_request(), y=after->pack_negotiate_request();
                    assert(x==y && before->method==after->method && std::memcmp(before->buf,after->buf,x)==0);
                    x=before->pack_auth_request();y=after->pack_auth_request();
                    assert(x==y && std::memcmp(before->buf,after->buf,x)==0);
                }
                ++socks;
            }
    for (size_t proxy_length : {0u,480u,481u}) for(size_t target_length : {0u,480u,505u}) {
        reference::Socks5Proxy before{};candidate::Socks5Proxy after{};
        before.dns_tunnel=after.dns_tunnel=1;
        before.host=after.host=std::string(proxy_length,'p');
        before.target_host=after.target_host=std::string(target_length,'d');
        before.target_port=after.target_port=-1;
        std::memset(before.buf,0xa5,sizeof(before.buf));std::memset(after.buf,0xa5,sizeof(after.buf));
        assert(before.pack_connect_request()==after.pack_connect_request());
        assert(std::memcmp(before.buf,after.buf,sizeof(before.buf))==0); // Failure prefix and untouched tail.
        ++socks;
    }
    for(auto value : {std::string(),std::string("OK"),std::string("\0binary",7),std::string("a\0z",3),std::string(65536,'b')}) {
        for(int type=0;type<=6;++type) {
            reference::String original;candidate::String actual;
            reference::redis::format(&original,reference::redis::ReplyType(type),value);
            candidate::redis::format(&actual,candidate::redis::ReplyType(type),value);
            assert(original.length==actual.length&&std::memcmp(original.str,actual.str,original.length)==0);++resp;
        }
    }
    for(long integer : {LONG_MIN,-1L,0L,1L,LONG_MAX}) for(int type=0;type<=6;++type) {
        reference::String original;candidate::String actual;
        reference::redis::format(&original,reference::redis::ReplyType(type),integer);
        candidate::redis::format(&actual,candidate::redis::ReplyType(type),integer);
        assert(original.length==actual.length&&std::memcmp(original.str,actual.str,original.length)==0);++resp;
    }
    for(std::string number : {"0","-1","+1"," 12","\v42","2147483648","4294967295","9223372036854775808","-9223372036854775809"}) {
        std::string command=":"+number+"\r\n";
        assert(reference::redis::parse(command.data(),command.size())==candidate::redis::parse(command.data(),command.size())); ++resp;
    }
    for (const std::string &digits : {std::string("9223372036854775807"),std::string("9223372036854775808"),
                                    std::string("-9223372036854775808"),std::string("-9223372036854775809"),
                                    std::string("0009223372036854775807"),std::string(4096,'9')}) {
        for (char prefix : {'*','$',':'}) for (const std::string &suffix : {std::string("\r\n"),std::string("x\r\n")}) {
            const std::string command = prefix + digits + suffix;
            for (int initial : {0, EDOM, ERANGE}) {
                errno = initial;
                // Original parse dereferences nullptr after a malformed numeric
                // delimiter; compare its defined get_number helper instead.
                if (suffix[0] == 'x') {
                    int value = 0;
                    assert(reference::redis::get_number(command.c_str(), &value) == nullptr);
                    const int before = errno;
                    errno = initial;
                    uint8_t range = 0;
                    assert(swr_v1_resp_parse_range(reinterpret_cast<const uint8_t *>(command.data()),command.size(),nullptr,0,&range) == -1);
                    if (range) errno = ERANGE;
                    assert(before == errno);
                    ++resp;
                    continue;
                }
                const auto expected = reference::redis::parse(command.data(),command.size());
                const int before = errno;
                errno = initial;
                const auto actual = candidate::redis::parse(command.data(),command.size());
                assert(expected == actual && before == errno);
                ++resp;
            }
        }
    }
    for (size_t arguments : {size_t(1),size_t(16),size_t(17),size_t(64),size_t(128)}) {
        std::string command = "*" + std::to_string(arguments) + "\r\n";
        for (size_t i=0;i<arguments;++i) command += "$1\r\nx\r\n";
        errno = EDOM;
        const auto expected = reference::redis::parse(command.data(),command.size());
        const int before = errno;
        errno = EDOM;
        assert(expected == candidate::redis::parse(command.data(),command.size()) && errno == before);
        ++resp;
    }
    for(std::string command : {"+OK\r\n","abc","x",":1\r\n$3\r\n","*1\r\n$3\r\nabc","$0\r\njunk","$-1\r\n:2\r\n"}) {
        assert(reference::redis::parse(command.data(),command.size())==candidate::redis::parse(command.data(),command.size())); ++resp;
    }
    for(size_t size : {size_t(0),size_t(1),size_t(125),size_t(4096),size_t(65536)}) {
        std::string payload(size,'x'); if(size)payload[size/2]=0;
        std::string command="*3\r\n$3\r\nSET\r\n$-1\r\n$"+std::to_string(size)+"\r\n"+payload+"\r\n";
        assert(reference::redis::parse(command.data(),command.size())==candidate::redis::parse(command.data(),command.size()));++resp;
    }
    for(std::string command : {"*1\r\n$3\r\nGET\r\n","*2\r\n$3\r\nSET\r\n$3\r\nabc\r\n","*1\r\n$10\r\nabc\r\n",":1\r\n","*1\r\n$-1\r\n", "*1\r\n:9223372036854775808\r\n", "*1\r\n$9223372036854775808x\r\n", "*-9223372036854775809\r\n"}) {
        for(size_t split=2;split<=command.size();++split) for(size_t maximum : {size_t(12),size_t(64),size_t(1048576)}) {
            reference::network::Socket a;candidate::network::Socket b;
            a.reads=b.reads={command.substr(0,split),command.substr(split)};
            if(split==command.size()){a.reads.pop_back();b.reads.pop_back();}
            reference::Connection x{nullptr,&a};candidate::Connection y{nullptr,&b};
            reference::String original;candidate::String actual;
            reference::Protocol p;candidate::Protocol q;p.package_max_length=q.package_max_length=maximum;
            std::vector<std::string> before,after;
            std::vector<int> before_errno,after_errno;
            p.onPackage=[&](auto *,auto *,auto *data){before_errno.push_back(errno);before.emplace_back(data->data,data->info.len);return 0;};
            q.onPackage=[&](auto *,auto *,auto *data){after_errno.push_back(errno);after.emplace_back(data->data,data->info.len);return 0;};
            errno = EDOM;
            const int first=reference::redis::recv_packet(&p,&x,&original);
            const int expected_errno = errno;
            errno = EDOM;
            const int second=candidate::redis::recv_packet(&q,&y,&actual);
            assert(expected_errno == errno && before_errno == after_errno);
            assert(first==second&&before==after&&original.length==actual.length&&original.offset==actual.offset);
            assert(std::memcmp(x.object,y.object,sizeof(swr_resp_request))==0);std::free(x.object);std::free(y.object);++receives;
        }
    }
    // New bounded ABI failures do not overwrite native output/state.
    std::array<uint8_t,16> output;output.fill(0xa5);const auto untouched=output;
    assert(swr_v1_mqtt_frame_length(nullptr,0)==0 && swr_v1_mqtt_frame_length(nullptr,1)==-1);
    assert(swr_v1_socks_auth(nullptr,1,nullptr,0,output.data(),output.size())==-1 && output==untouched);
    assert(swr_v1_socks_auth(nullptr,0,nullptr,0,output.data(),2)==-1 && output==untouched);
    assert(swr_v1_socks_connect(1,output.data(),3,80,output.data(),output.size())==-1 && output==untouched);
    assert(swr_v1_resp_format(0,nullptr,1,0,output.data(),output.size())==0 && output==untouched);
    assert(swr_v1_resp_format(8,nullptr,0,LONG_MIN,output.data(),1)==0 && output==untouched);
    swr_resp_span span{99,77,42,3};
    const auto *number=reinterpret_cast<const uint8_t *>(":1\r\n");
    assert(swr_v1_resp_parse(number,4,&span,0)==-2 && span.offset==99 && span.kind==3);
    swr_resp_request request{};uint8_t offset=99;
    assert(swr_v1_resp_receive(number,4,3,100,&request,&offset)==-1 && offset==99 && request.state==0);
    uint64_t corpus=0;
    resp += seeded_resp_trace(&corpus);
    const auto handlers=handler_key_trace();
    std::printf("{\"mqtt\":%zu,\"socks\":%zu,\"resp\":%zu,\"receive_states\":%zu,\"handler_keys\":%zu,\"errors\":0,\"resp_corpus_fnv1a64\":\"%016" PRIx64 "\"}\n",mqtt,socks,resp,receives,handlers,corpus);
}
