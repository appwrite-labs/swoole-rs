#include <algorithm>
#include <cassert>
#include <cstdint>
#include <iostream>
#include <string>
#include <sys/types.h>
#include "swoole_rs_reactor.h"
struct Scenario{std::string input,output;size_t piece=1;unsigned file=0,ssl_file=0,getters=0,sync=0,queue=0,queries=0;bool active=false;}scenario;
namespace swoole {
struct File{int get_fd()const{++scenario.getters;return 18;}};
static ssize_t file_effect(off_t *offset,size_t length){assert(length>0&&*offset>=0&&static_cast<size_t>(*offset)+length<=scenario.input.size());size_t n=std::min(length,scenario.piece);scenario.output.append(scenario.input.data()+*offset,n);*offset+=n;return n;}
static ssize_t swoole_sendfile(int out,int in,off_t *offset,size_t length){assert(out==17&&in==18);++scenario.file;return file_effect(offset,length);}
static bool swoole_event_is_available(){++scenario.queries;return scenario.active;}
struct Socket;
static ssize_t swoole_event_write(Socket*,const void*,size_t);
struct Socket {
 int fd=17;void *ssl=nullptr;
 ssize_t ssl_sendfile(const File&,off_t *offset,size_t length){++scenario.ssl_file;return file_effect(offset,length);}
 ssize_t send_sync(const void *data,size_t length,int flags){assert(length>0&&flags==0);++scenario.sync;scenario.output.append(static_cast<const char*>(data),length);return length;}
 ssize_t sendfile(const File&,off_t*,size_t);ssize_t send_async(const void*,size_t);void clean()const;
};
static ssize_t swoole_event_write(Socket *socket,const void *data,size_t length){assert(socket&&length>0);++scenario.queue;scenario.output.append(static_cast<const char*>(data),length);return static_cast<ssize_t>(length);}
#include "forwarding.inc"
}
int main(){using namespace swoole;unsigned cases=0,checks=0;auto check=[&](bool value){assert(value);++checks;};std::string payload;for(unsigned i=0;i<47;++i)payload+=static_cast<char>(i);
for(off_t offset:{off_t(0),off_t(2)})for(size_t length:{size_t(1),size_t(7),size_t(19)})for(size_t piece:{size_t(1),size_t(3),size_t(21)})for(bool tls:{false,true}){scenario=Scenario();scenario.input=payload;scenario.piece=piece;Socket s;s.ssl=tls?&s:nullptr;File file;off_t position=offset;size_t expected=std::min(length,piece);check(s.sendfile(file,&position,length)==static_cast<ssize_t>(expected));check(position==offset+static_cast<off_t>(expected));check(scenario.output==payload.substr(offset,expected));check(scenario.file==unsigned(!tls)&&scenario.ssl_file==unsigned(tls));check(scenario.getters==unsigned(!tls));check(scenario.sync==0&&scenario.queue==0&&scenario.queries==0);check(s.fd==17&&s.ssl==(tls?&s:nullptr));++cases;}
for(size_t length:{size_t(1),size_t(7),size_t(31)})for(bool active:{false,true}){scenario=Scenario();scenario.active=active;Socket s;check(s.send_async(payload.data(),length)==static_cast<ssize_t>(length));check(scenario.output==payload.substr(0,length));check(scenario.queries==1);check(scenario.sync==unsigned(!active)&&scenario.queue==unsigned(active));check(scenario.file==0&&scenario.ssl_file==0&&scenario.getters==0);check(s.fd==17&&s.ssl==nullptr);++cases;}
std::cout<<"{\"cases\":"<<cases<<",\"checks\":"<<checks<<",\"positive_only\":true,\"clean_native_executed\":false}\n";
}
