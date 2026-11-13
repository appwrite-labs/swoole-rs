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
#define swoole_trace_log(...) ((void)0)
#define swoole_trace(...) ((void)0)
#define SW_MQTT_MIN_LENGTH_SIZE 1
#define SW_MQTT_MAX_LENGTH_SIZE 4
#define SW_MQTT_RECV_LEN_AGAIN 0
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
}
int main() {
    size_t mqtt = 0, socks = 0;
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
    std::printf("{\"mqtt\":%zu,\"socks\":%zu,\"errors\":0}\n",mqtt,socks);
}
