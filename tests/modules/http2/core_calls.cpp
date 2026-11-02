#include "swoole_socket.h"
#include "swoole_http2.h"
#include "swoole_protocol.h"
#include "rust_http2.h"
#include <array>
#include <iostream>
#include <vector>
#include <climits>

namespace swoole { namespace http2 {
void original_put_default_setting(swHttp2SettingId, uint32_t);
uint32_t original_get_default_setting(swHttp2SettingId);
size_t original_pack_setting_frame(char *, const Settings &, bool);
ReturnCode original_unpack_setting_data(const char *, ssize_t, const std::function<ReturnCode(uint16_t,uint32_t)> &);
ssize_t original_get_frame_length(const Protocol *, network::Socket *, PacketLength *);
int original_send_setting_frame(Protocol *, network::Socket *);
const char *original_get_type(int);
int original_get_type_color(int);
} }
static std::vector<char> emitted;
static ssize_t send_result;
namespace swoole { namespace network {
ssize_t Socket::send(const void *bytes, size_t size, int flags) {
    if (flags != 0) std::abort();
    emitted.assign((const char *)bytes, (const char *)bytes + size);
    return send_result;
}
} }
#define CHECK(x) do { if (!(x)) { std::cerr << "FAIL line=" << __LINE__ << " " #x << std::endl; return 1; } } while (0)
int main() {
    using namespace swoole; namespace H = swoole::http2;
    uint32_t state=0x8e8c4991;size_t cases=0;
    auto random=[&](){state^=state<<13;state^=state>>17;state^=state<<5;return state;};
    for (size_t iteration=0;iteration<5000;++iteration) {
        H::Settings settings={random(),random(),random(),random(),random(),random()};
        const std::array<uint32_t,6> fields={settings.header_table_size, settings.enable_push, settings.max_concurrent_streams,
            settings.init_window_size, settings.max_frame_size, settings.max_header_list_size};
        for (uint32_t id=1;id<=6;++id) {
            H::put_default_setting((swHttp2SettingId)id,fields[id-1]);
            H::original_put_default_setting((swHttp2SettingId)id,fields[id-1]);
            CHECK(H::get_default_setting((swHttp2SettingId)id)==H::original_get_default_setting((swHttp2SettingId)id));++cases;
        }
        for (bool server: {false,true}) {
            std::array<char,64> original{},rust{};
            auto a=H::original_pack_setting_frame(original.data(),settings,server);
            auto b=H::pack_setting_frame(rust.data(),settings,server);
            CHECK(a==b && original==rust);++cases;
            PacketLength pl{};pl.buf=original.data();
            for (size_t size: {0u,1u,8u,9u,39u,45u}) {
                pl.buf_size=size;CHECK(H::get_frame_length(nullptr,nullptr,&pl)==H::original_get_frame_length(nullptr,nullptr,&pl));++cases;
            }
            // Original callback is executed before the next item and may rewrite it.
            for (int stop: {0,1,3,8}) {
                auto execute=[&](bool native) {
                    auto data=original;std::vector<uint64_t> trace;size_t index=0;
                    auto callback=[&](uint16_t id,uint32_t value)->ReturnCode {
                        trace.push_back((uint64_t(id)<<32)|value);
                        if (++index==1) data[17]=char(0x5a);
                        return int(index)==stop ? SW_ERROR : SW_SUCCESS;
                    };
                    auto result=native?H::original_unpack_setting_data(data.data()+9,a-9,callback):H::unpack_setting_data(data.data()+9,a-9,callback);
                    trace.push_back(uint32_t(result));return trace;
                };
                CHECK(execute(true)==execute(false));++cases;
            }
        }
    }
    // Signed nonpositive lengths do not touch the pointer or invoke callback.
    for (ssize_t length: {-17,-1,0}) {
        int calls=0;auto cb=[&](uint16_t,uint32_t)->ReturnCode{++calls;return SW_ERROR;};
        CHECK(H::original_unpack_setting_data(nullptr,length,cb)==H::unpack_setting_data(nullptr,length,cb));CHECK(calls==0);++cases;
    }
    // Positive partial logical lengths preserve a valid initialized six-byte backing item.
    for (ssize_t length: {1,2,3,4,5,6,7,11,12}) {
        std::array<char,24> data{};for(size_t i=0;i<data.size();++i)data[i]=char(i+1);
        auto execute=[&](bool native){std::vector<uint64_t> trace;auto cb=[&](uint16_t id,uint32_t v)->ReturnCode{trace.push_back((uint64_t(id)<<32)|v);return SW_SUCCESS;};
            auto code=native?H::original_unpack_setting_data(data.data(),length,cb):H::unpack_setting_data(data.data(),length,cb);trace.push_back(uint32_t(code));return trace;};
        CHECK(execute(true)==execute(false));++cases;
    }
    for(int type=-100;type<100;++type){CHECK(!strcmp(H::get_type(type),H::original_get_type(type)));CHECK(H::get_type_color(type)==H::original_get_type_color(type));cases+=2;}
    for(int type:{INT_MIN,INT_MAX}){CHECK(!strcmp(H::get_type(type),H::original_get_type(type)));CHECK(H::get_type_color(type)==H::original_get_type_color(type));cases+=2;}
    network::Socket socket{};
    for(ssize_t result:{-1,0,1,39}) {
        send_result=result;CHECK(H::send_setting_frame(nullptr,&socket)==result);auto rust=emitted;
        CHECK(H::original_send_setting_frame(nullptr,&socket)==result);CHECK(rust==emitted);++cases;
    }
    std::cout << "{\"passed\":true,\"actual_core_cases\":" << cases << "}" << std::endl;
}
