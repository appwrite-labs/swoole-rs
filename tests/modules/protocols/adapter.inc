// Native allocation/socket model only. Pinned protocol bodies are unchanged.
using uchar = unsigned char;
constexpr int SW_OK = 0, SW_ERR = -1, SW_ERROR = 10, SW_CLOSE = 11, SW_REDUCE_SIZE = 12;
enum ReturnCode { SW_CONTINUE = 1, SW_WAIT = 2, SW_READY = 3 };
enum SocketType { SW_SOCK_TCP = 1, SW_SOCK_UDP = 2, SW_SOCK_TCP6 = 3, SW_SOCK_UDP6 = 4 };
struct String {
    inline static size_t live = 0;
    size_t length = 0, size = 0;
    off_t offset = 0;
    char *str = nullptr;
    String() { reserve(64); ++live; }
    ~String() { std::free(str); --live; }
    String(const String &) = delete;
    String &operator=(const String &) = delete;
    String(const char *data, size_t length) : String() { append(data, length); }
    void reserve(size_t capacity) { auto next = static_cast<char *>(std::realloc(str, capacity)); if (!next) throw std::bad_alloc(); str = next; size = capacity; }
    void extend(size_t capacity) { assert(capacity > size); reserve(capacity); }
    void append(const char *data, size_t count) {
        if (length + count > size) reserve(length + count);
        if (count) std::memcpy(str + length, data, count);
        length += count;
    }
    void append(const std::string &value) { append(value.data(), value.size()); }
    void format(const char *pattern, ...) {
        va_list arguments, copy; va_start(arguments, pattern); va_copy(copy, arguments);
        int count = std::vsnprintf(nullptr, 0, pattern, copy); va_end(copy);
        std::vector<char> result(count + 1); std::vsnprintf(result.data(), result.size(), pattern, arguments);
        va_end(arguments); append(result.data(), count);
    }
    void clear() { length = 0; offset = 0; }
};
struct DataHead { uint64_t msg_id; uint32_t len; uint8_t flags, type; };
struct RecvData { DataHead info{}; const char *data = nullptr; };
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
    std::vector<std::string> reads;
    size_t read_index = 0, read_offset = 0;
    bool removed = false;
    static bool is_inet4(SocketType type) { return type == SW_SOCK_TCP || type == SW_SOCK_UDP; }
    static bool is_inet6(SocketType type) { return type == SW_SOCK_TCP6 || type == SW_SOCK_UDP6; }
    int catch_read_error(int) { return SW_WAIT; }
    ssize_t recv(char *output, size_t capacity, int) {
        if (read_index == reads.size()) { errno = EAGAIN; return -1; }
        size_t size = std::min(capacity, reads[read_index].size() - read_offset);
        if (!size) return 0;
        std::memcpy(output, reads[read_index].data() + read_offset, size);
        read_offset += size;
        if (read_offset == reads[read_index].size()) { ++read_index; read_offset = 0; }
        return size;
    }
};
}
using network::Socket;
struct Connection { void *object = nullptr; network::Socket *socket; int fd = 3; };
struct Protocol {
    uint32_t package_max_length = 1 << 20;
    std::function<int(Protocol *, network::Socket *, RecvData *)> onPackage;
};
static size_t swoole_size_align(size_t size, size_t align) { return (size + align - 1) / align * align; }
static size_t swoole_pagesize() { return 4096; }
