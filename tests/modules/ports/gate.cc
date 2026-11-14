#include "swoole_rs_ports.h"
#include <algorithm>
#include <climits>
#include <cmath>
#include <cstdint>
#include <cstring>
#include <iostream>
#include <limits>
#include <strings.h>
#include <string>
#include <unordered_map>
#include "reference.inc"

static uint64_t checks;
static void check(bool ok) { if(!ok){std::cerr<<"failed check "<<checks<<"\n";std::abort();}checks++; }
static void event(const std::string &name,bool folded) {
    int expected=-1;std::string property;
    for(const auto &entry: server_port_event_map) {
        if(folded ? swoole_strcaseeq(name.data(),name.size(),entry.first.data(),entry.first.size()) : name==entry.first) {
            expected=entry.second.type;property="on"+entry.second.name;break;
        }
    }
    auto value=swr_v1_ports_event(reinterpret_cast<const uint8_t *>(name.data()),name.size(),folded);
    check(value.index==expected);
    if(expected>=0)check(std::string(reinterpret_cast<const char *>(value.property.data),value.property.len)==property);
    else check(value.property.data==nullptr&&value.property.len==0);
}
int main() {
    static_assert(sizeof(SwrPortsSpan)==16&&sizeof(SwrPortsEvent)==24,"ports 64-bit span ABI");
    for(const auto &entry:server_port_event_map) {
        event(entry.first,false);event(entry.first,true);
        auto upper=entry.first;for(char &c:upper)c=char(c>='a'&&c<='z'?c-32:c);
        event(upper,true);event(upper,false);
        for(size_t n=0;n<entry.first.size();n++)for(unsigned c=0;c<256;c++) {
            auto changed=entry.first;changed[n]=char(c);event(changed,true);event(changed,false);
        }
    }
    event("",true);event("unknown",false);event(std::string("connect\0",8),true);
    auto missing=swr_v1_ports_event(nullptr,0,1);check(missing.index==-1);
    std::cout<<"{\"checks\":"<<checks<<",\"errors\":0}"<<std::endl;
}
