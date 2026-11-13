// Native address model only. Pinned protocol bodies are unchanged.
using uchar = unsigned char;
constexpr int SW_ERR = -1;
enum SocketType { SW_SOCK_TCP = 1, SW_SOCK_UDP = 2, SW_SOCK_TCP6 = 3, SW_SOCK_UDP6 = 4 };
struct PacketLength { const char *buf; uint32_t buf_size; uint32_t header_len; };
namespace network {
struct Address {
    union { sockaddr_in inet_v4; sockaddr_in6 inet_v6; } addr{};
    bool assign(SocketType type, const std::string &host, int port, bool) {
        if (port < 0 || port > 65535) return false;
        if (type == SW_SOCK_TCP || type == SW_SOCK_UDP) return inet_pton(AF_INET, host.c_str(), &addr.inet_v4.sin_addr) == 1;
        return inet_pton(AF_INET6, host.c_str(), &addr.inet_v6.sin6_addr) == 1;
    }
};
struct Socket {
    static bool is_inet4(SocketType type) { return type == SW_SOCK_TCP || type == SW_SOCK_UDP; }
    static bool is_inet6(SocketType type) { return type == SW_SOCK_TCP6 || type == SW_SOCK_UDP6; }
};
}
using network::Socket;
struct Protocol;
