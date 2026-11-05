#include "swoole_socket.h"
#include <cassert>
#include <cerrno>
#include <climits>
#include <cstring>
#include <thread>
using swoole::network::Address;

int main() {
    Address v4{}, v6{}, local{};
    assert(v4.empty());
    assert(v4.assign(SW_SOCK_UDP, "127.0.0.1", 80, false));
    assert(v6.assign(SW_SOCK_TCP6, "::1", 443, false));
    assert(local.assign(SW_SOCK_UNIX_DGRAM, "/tmp/address-record", 0, false));
    const char *scratch = v4.get_addr();
    assert(std::strcmp(scratch, "127.0.0.1") == 0);
    assert(v6.get_addr() == scratch);
    assert(std::strcmp(scratch, "::1") == 0);
    assert(Address::verify_ip(AF_INET, "127.0.0.1"));
    const unsigned char loopback[] = {127, 0, 0, 1};
    assert(std::memcmp(scratch, loopback, sizeof(loopback)) == 0);
    assert(v4.get_addr() == scratch);
    std::thread other([&] {
        assert(v6.get_addr() != scratch);
        assert(Address::verify_ip(AF_INET6, "fe80::1"));
    });
    other.join();
    assert(std::strcmp(scratch, "127.0.0.1") == 0);
    assert(local.get_addr() == local.addr.un.sun_path);
    Address copy = local;
    local.addr.un.sun_path[1] = 'X';
    assert(std::strcmp(copy.get_addr(), "/tmp/address-record") == 0);
    assert(v4.is_loopback_addr() && v6.is_loopback_addr());
    assert(v4.assign(SW_SOCK_TCP, "127.0.0.2", 0, false));
    assert(!v4.is_loopback_addr());
    for (Address *address : {&v4, &v6}) {
        for (int port : {INT_MIN, -65537, -1, 0, 1, 65535, 65536, INT_MAX}) {
            address->set_port(port);
            assert(address->get_port() == static_cast<uint16_t>(port));
        }
    }
    copy.set_port(65535);
    assert(copy.get_port() == 0);
    assert(std::strcmp(copy.get_addr(), "/tmp/address-record") == 0);
    Address unknown{};
    unknown.type = static_cast<swSocketType>(99);
    assert(!unknown.empty() && unknown.get_port() == 0);
    assert(std::strcmp(unknown.get_addr(), "unknown") == 0);
    assert(std::strcmp(Address::type_str(SW_SOCK_RAW), "IPv4") == 0);
    assert(std::strcmp(Address::type_str(SW_SOCK_RAW6), "IPv6") == 0);
    assert(std::strcmp(Address::type_str(SW_SOCK_UNIX_STREAM), "UnixSocket") == 0);
    assert(std::strcmp(Address::type_str(unknown.type), "Unknown") == 0);
    errno = 0;
    assert(Address::addr_str(AF_UNIX, &v4.addr) == nullptr);
    assert(errno == EAFNOSUPPORT);
}
