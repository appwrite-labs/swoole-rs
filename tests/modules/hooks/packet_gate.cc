#include <algorithm>
#include <cassert>
#include <cerrno>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <iomanip>
#include <functional>
#include <limits>
#include <sstream>
#include <string>
#include <vector>
#include "swoole_socket.h"
#define SWOOLE_RS_REACTOR 1
#include "swoole_rs_socket.h"
#include "swoole_rs_co_socket.h"
static std::vector<std::string> trace;
static void record(std::string value){trace.push_back(value);}
static std::string number(double value){std::ostringstream out;out<<std::hexfloat<<value;return out.str();}
#define SW_EVENT_READ 512
#define SW_EVENT_WRITE 1024
#define SW_WAIT 2
using EventBarrier=std::function<bool()>;
#undef SW_BUFFER_SIZE_STD
#define SW_BUFFER_SIZE_STD 4
#undef SW_BUFFER_SIZE_BIG
#define SW_BUFFER_SIZE_BIG 8
#define SW_ERROR_PACKAGE_LENGTH_NOT_FOUND 1781
#define SW_ERROR_PACKAGE_LENGTH_TOO_LARGE 1782
#undef sw_unlikely
#define sw_unlikely(value) (value)
#undef swoole_trace_log
#define swoole_trace_log(kind,format,result,length) record("trace:"+std::to_string(result)+":"+std::to_string(length))
#undef swoole_error_log
#define swoole_error_log(level,error,format,address,port,length) record("diagnostic:"+std::string(address)+":"+std::to_string(port)+":"+std::to_string(length))
#define ACTUAL_BODIES "original-packet.inc"
namespace original {
#include "packet_fixture.h"
}
#undef ACTUAL_BODIES
#define ACTUAL_BODIES "selected-packet.inc"
namespace selected {
#include "packet_fixture.h"
}
#define CASES(NS) \
static std::vector<std::string> run_##NS(int scenario){using namespace NS;trace.clear();errno=0;packet_lengths={6};packet_index=read_index=input_index=0;reads={2,4,4,4,4,4};input="abcdefghijklmnop";next_header=4;timer_ok=true;available=true;wake_ok=true; \
 if(scenario<240){Socket socket;size_t seed=0,capacity=4;int mode=scenario%12;int variant=(scenario/12)%8; \
 if(scenario<96 || (scenario>=216 && scenario<228)){socket.open_length_check=true;socket.open_eof_check=variant&2;timer_ok=!(variant&4);if(variant&1){seed=1;socket.protocol.package_length_size=0;socket.protocol.package_length_type='\0';}if(mode==1)packet_lengths={-1};if(mode==2){packet_lengths={0,6};reads={2,2,2};}if(mode==3)packet_lengths={24};if(mode==4)packet_lengths={2};if(mode==5)reads={2,2,0};if(mode==6)reads={2,-EIO};if(mode==7)reads={0};if(mode==8)reads={-EIO};if(mode==9){seed=6;capacity=8;}if(mode==10)socket.protocol.package_max_length=5;if(mode==11){seed=1;socket.protocol.package_length_size=0;socket.protocol.package_length_type='\0';packet_lengths={1};}} \
 else if(scenario<192 || scenario>=228){socket.open_eof_check=true;timer_ok=!(variant&4);if(variant&1)seed=1;if(variant&2)socket.open_eof_check=false;input="ab\r\ncd";reads={2,2,2,2,2,2};if(mode==1)reads={1,1,1,1,1,1};if(mode==2){input="abcdefghijklmnop";socket.protocol.package_max_length=8;}if(mode==3){input="abcdef\r\n";reads={4,4};}if(mode==4){input="";seed=0;reads={0};}if(mode==5)reads={-EIO};if(mode==6)reads={2,-EIO};if(mode==7){input="ab\ncd";socket.protocol.package_eof[0]='\n';socket.protocol.package_eof_len=1;}if(mode==8){input=std::string("ab\0!cd",6);socket.protocol.package_eof[0]='\0';socket.protocol.package_eof[1]='!';}if(mode==9){input="ab\r\ncd";seed=6;capacity=8;}if(mode==10){input="abcdefghij";socket.protocol.package_max_length=5;reads={4,4,4};}if(mode==11){input="abcdefghij";std::memcpy(socket.protocol.package_eof,"abcdef",6);socket.protocol.package_eof_len=6;reads={4,4,4};}} \
 else{int k=scenario-192;input="abcdefgh";reads=k%3==0?std::vector<ssize_t>{3}:k%3==1?std::vector<ssize_t>{0}:std::vector<ssize_t>{-EIO};if(k&4){seed=2;capacity=8;}available=!(k&8);timer_ok=!(k&16);} \
 if(seed || scenario>=216){socket.read_buffer=new String(capacity);seed=std::min(seed,input.size());std::memcpy(socket.read_buffer->str,input.data(),seed);socket.read_buffer->length=seed;input_index=seed;if(scenario>=192 && scenario<216 && seed)socket.read_buffer->offset=1;} \
 ssize_t result=socket.recv_packet(0);record("result:"+std::to_string(result));record("errno:"+std::to_string(errno));if(socket.read_buffer){auto *buffer=socket.read_buffer;record("buffer:"+std::to_string(buffer->length)+":"+std::to_string(buffer->offset)+":"+std::to_string(buffer->size));std::ostringstream bytes;for(size_t i=0;i<buffer->length;++i)bytes<<std::hex<<std::setw(2)<<std::setfill('0')<<static_cast<unsigned>(static_cast<unsigned char>(buffer->str[i]));record("bytes:"+bytes.str());}} \
 if(scenario>=240){int k=scenario-240;int method=k%2;int mode=(k/2)%12;int failure=(k/24)%4;int layout=k/96;std::vector<std::vector<ssize_t>> results={{0},{8},{-EINVAL},{-EAGAIN,8},{2,3,3},{2,0},{2,-EAGAIN,3,3},{2,-EAGAIN,-EIO},{-EAGAIN,0},{-EINTR},{2,2,-EAGAIN,4},{2,2,4}};reads=results[mode];std::vector<std::vector<size_t>> layouts={{8},{2,3,3},{0,3,0,5},{}};network::IOVector vector{layouts[layout]};available=failure!=1;timer_ok=failure!=2;wake_ok=failure!=3;Socket socket;socket.errCode=9;auto result=method==0?socket.readv_all(&vector):socket.writev_all(&vector);record("result:"+std::to_string(result));record("errno:"+std::to_string(errno));record("error:"+std::to_string(socket.errCode));record("remaining:"+std::to_string(vector.get_remain_count()));record("barriers:"+std::to_string(socket.recv_barrier!=nullptr)+":"+std::to_string(socket.send_barrier!=nullptr));} \
 return trace;}
CASES(original)
CASES(selected)
// Upstream's EOF search (swoole_strnpos) and String::clear are pure steps the selected
// Rust performs itself; their effects show in the final buffer state, not as calls.
static std::vector<std::string> effects(std::vector<std::string> lines){lines.erase(std::remove_if(lines.begin(),lines.end(),[](const std::string &line){return line.rfind("scan:",0)==0||line=="buffer:clear";}),lines.end());return lines;}
int main(){for(int scenario=0;scenario<624;++scenario){auto a=effects(run_original(scenario)),b=effects(run_selected(scenario));if(a!=b){std::fprintf(stderr,"packet mismatch %d\n",scenario);for(auto &line:a)std::fprintf(stderr,"original %s\n",line.c_str());for(auto &line:b)std::fprintf(stderr,"selected %s\n",line.c_str());return 1;}}std::puts("624 actual pinned/selected vector/packet framing native-facade scenarios passed");}
