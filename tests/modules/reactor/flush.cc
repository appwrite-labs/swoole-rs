#include <cassert>
#include <cstdint>
#include <deque>
#include <iostream>
#include <string>
#include <vector>
#include "swoole_rs_reactor.h"
constexpr int SW_OK=0, SW_ERR=-1, SW_EVENT_WRITE=1<<10;
static std::vector<std::string> effects;
struct BufferChunk { enum Type { TYPE_DATA, TYPE_SENDFILE, TYPE_CLOSE }; Type type; };
extern "C" {
struct swr_v1_buffer_typed_detached { void *token; void (*dispose)(void *); };
int32_t swr_v1_buffer_typed_create(uint32_t, swr_v1_buffer **);
int32_t swr_v1_buffer_append(swr_v1_buffer *, const uint8_t *, size_t);
int32_t swr_v1_buffer_typed_pop(swr_v1_buffer *, swr_v1_buffer_typed_detached *);
int32_t swr_v1_buffer_release(swr_v1_buffer *);
}
// The selected bodies read emptiness from the Rust queue, so it mirrors the chunk list.
struct Buffer {
    std::deque<BufferChunk> chunks;
    swr_v1_buffer *rs = nullptr;
    Buffer() { assert(swr_v1_buffer_typed_create(1, &rs) == 0); }
    ~Buffer() { swr_v1_buffer_release(rs); }
    void push(BufferChunk::Type type) { chunks.push_back({type}); assert(swr_v1_buffer_append(rs, reinterpret_cast<const uint8_t *>("x"), 1) == 0); }
    void pop() { chunks.pop_front(); swr_v1_buffer_typed_detached detached{}; assert(swr_v1_buffer_typed_pop(rs, &detached) == 0); }
    void clear() { while (!chunks.empty()) pop(); }
    swr_v1_buffer *handle() const { return rs; }
    static bool empty(Buffer *b) { return !b || b->chunks.empty(); }
    BufferChunk *front() { return &chunks.front(); }
    struct Front { int kind; };
    Front typed_front() const { return {chunks.front().type}; }
};
// swoole::network::Socket's write-path block layout, as the selected bodies expect.
struct Socket {
    Buffer *out_buffer;
    void *send_timer=nullptr;
    uint32_t buffer_size=0;
    uint8_t close_wait=0, nonblock=1, ssl_send_=0, send_wait=0;
    int fd=7;
    bool removed=false;
    static constexpr double default_write_timeout=1;
    swr_v1_socket_output *output() { return reinterpret_cast<swr_v1_socket_output *>(&out_buffer); }
    int handle_send() { effects.emplace_back("data"); out_buffer->pop(); return SW_OK; }
    int handle_sendfile() { effects.emplace_back("sendfile"); out_buffer->pop(); return SW_OK; }
    int wait_event(int, int event) { assert(event==SW_EVENT_WRITE); effects.emplace_back("wait"); return SW_OK; }
};
struct Event { Socket *socket; int fd; };
static double sec2msec(double seconds) { return seconds*1000; }
static void swoole_timer_del(void *timer) { assert(timer); effects.emplace_back("timer"); }
#include "flush.inc"
template<class Reactor> std::vector<std::string> run(std::initializer_list<BufferChunk::Type> kinds, bool timer, bool drain) {
    effects.clear(); Buffer buffer; for (auto type:kinds) buffer.push(type);
    Socket socket; socket.out_buffer=&buffer; if(timer) socket.send_timer=&socket;
    Reactor reactor; Event event{&socket, socket.fd};
    if(drain) reactor.drain_write_buffer(&socket);
    else assert(Reactor::_writable_callback(&reactor,&event)==SW_OK);
    effects.push_back("remaining="+std::to_string(buffer.chunks.size()));
    effects.push_back("removed="+std::to_string(socket.removed));
    effects.push_back("timer="+std::to_string(socket.send_timer!=nullptr));
    return effects;
}
int main() {
    unsigned count=0;
    for(bool timer:{false,true}) for(bool drain:{false,true}) {
        for(auto types:{std::initializer_list<BufferChunk::Type>{}, {BufferChunk::TYPE_DATA}, {BufferChunk::TYPE_SENDFILE}, {BufferChunk::TYPE_DATA,BufferChunk::TYPE_SENDFILE,BufferChunk::TYPE_DATA}, {BufferChunk::TYPE_CLOSE}, {BufferChunk::TYPE_DATA,BufferChunk::TYPE_CLOSE}}) {
            const auto original=run<ReactorOriginal>(types,timer,drain);
            const auto selected=run<ReactorSelected>(types,timer,drain);
            assert(original==selected); ++count;
        }
    }
    std::cout<<"{\"positive_cases\":"<<count<<",\"native_effect_trace_pairs\":"<<count<<",\"data_destroy_callbacks\":false,\"native_socket_syscalls\":false}\n";
}
