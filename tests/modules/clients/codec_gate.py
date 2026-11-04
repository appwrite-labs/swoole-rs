#!/usr/bin/env python3
"""Actual pinned/patched DNS domain codec bodies on defined bounded buffers.

Whole query/response callers additionally use IO doubles and aligned records.
This is not live DNS, c-ares or resolver runtime parity.
"""
import argparse
import json
from pathlib import Path
import gate

ROOT=Path(__file__).resolve().parents[3]

def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--source',required=True,type=Path)
    p.add_argument('--archive',type=Path,required=True)
    p.add_argument('--output',required=True,type=Path)
    p.add_argument('--sanitize',action='store_true')
    p.add_argument('--rust-sanitized',action='store_true')
    a=p.parse_args();out=a.output.parent/(a.output.stem+'-artifacts');out.mkdir(parents=True,exist_ok=True);gate.LOG=out/'commands';gate.LOG.mkdir(exist_ok=True)
    if a.rust_sanitized and not a.sanitize:p.error('--rust-sanitized needs --sanitize')
    archive=a.archive
    original,selected=gate.sources(a.source,archive,'src/network/dns.cc')
    encoder='static int domain_encode(const char *src, int n, char *dest) {'
    decoder_native='static void domain_decode(char *str) {'
    native=gate.body(original,encoder)+'\n'+gate.body(original,decoder_native)
    # The patched lookup calls the Rust codec directly; these are the deleted
    # wrappers' bounds over the same exports, so the codec still meets upstream's.
    rust=r'''
static int domain_encode(const char *src, int n, char *dest) {
    constexpr size_t capacity = SW_BUFFER_SIZE_STD - 12;
    if (n < 0 || static_cast<size_t>(n) > capacity - 3 || src[n] == '.') return SW_ERR;
    return swr_v1_clients_domain_encode(reinterpret_cast<const uint8_t *>(src), n, reinterpret_cast<uint8_t *>(dest), capacity);
}
static void domain_decode(char *str, size_t capacity) {
    swr_v1_clients_domain_decode(reinterpret_cast<uint8_t *>(str), capacity, std::numeric_limits<char>::is_signed);
}
'''
    prelude=r'''
#include <cstring>
#include <cassert>
#include <cstdint>
#include <string>
#include <vector>
#include <iostream>
#include <algorithm>
#include <cstdio>
#include <cerrno>
#include <arpa/inet.h>
#include <limits>
#include "swoole_rs_clients.h"
#define SW_ERR -1
#define SW_OK 0
#define SW_BUFFER_SIZE_STD 8192
using uchar=unsigned char;
'''
    prelude += '\n'.join(gate.body(original, signature)+';' for signature in ['struct RecordHeader {', 'struct Q_FLAGS {', 'struct RR_FLAGS {'])+'\n'
    prelude += r'''
#define SW_SOCK_UDP 1
#define SW_DNS_A_RECORD 1
#define SW_DNS_AAAA_RECORD 28
#define SW_ERROR_DNSLOOKUP_NO_SERVER 714
#define SW_ERROR_DNSLOOKUP_RESOLVE_FAILED 711
#define SW_ERROR_CO_CANCELED 10017
namespace swoole {struct DnsServer {std::string host;int port;};}
#define sw_memset_zero(p,n) std::memset(p,0,n)
#define sw_snprintf std::snprintf
#define swoole_warning(...) ((void)0)
struct {struct {std::string host="127.0.0.1";int port=53;} dns_server;} SwooleG;
static bool swoole_load_resolv_conf(){return false;}
static int last_error=0,record_type=1,receive_mode=2;
static bool send_success=true,canceled=false,bad_id=false;
static std::vector<char> sent_query;
static std::vector<std::string> io_effects;
static void swoole_set_last_error(int value){last_error=value;io_effects.push_back("error:"+std::to_string(value));}
struct Socket {
    int errCode=canceled?ECANCELED:EIO;
    explicit Socket(int){}
    void set_timeout(double){io_effects.push_back("timeout");}
    int sendto(const std::string&,int,const char *data,size_t length){sent_query.assign(data,data+length);io_effects.push_back("send");return send_success;}
    long recv(void *buffer,size_t capacity){auto data=static_cast<char *>(buffer);
        io_effects.push_back("receive");if(receive_mode==0)return -1;
        auto response=sent_query;response[6]=0;response[7]=1;if(bad_id)response[0]^=1;
        std::vector<unsigned char> payload;
        if(record_type==1)payload={127,0,0,1};
        else if(record_type==28)payload={0x20,1,0x0d,0xb8,0,0,0,0,0,0,0,0,0,0,0,1};
        else payload={5,'a','l','i','a','s',4,'t','e','s','t',0};
        for(unsigned char byte:std::vector<unsigned char>{0xc0,0x0c,0,(unsigned char)record_type,0,1,0,0,0,0,0,(unsigned char)payload.size()})response.push_back(byte);
        for(auto byte:payload)response.push_back(byte);
        size_t length=receive_mode==1?3:response.size();if(length>capacity)throw std::runtime_error("fixture capacity");std::memcpy(data,response.data(),length);return length;
    }
};
'''
    parser_signature='std::vector<std::string> dns_lookup_impl_with_socket('
    formatter_signature='static std::string parse_ip_address(void *vaddr, int type) {'
    native += '\nstatic uint16_t dns_request_id=1;\n'+gate.body(original,formatter_signature)+'\n'+gate.body(original,parser_signature)
    # The hybrid's DNS server is Rust state; the driver sets it to SwooleG's.
    rust += '\nstatic uint16_t dns_request_id=1;\n'+'\n'.join(gate.body(selected,signature) for signature in ['static void assign_text(','static swoole::DnsServer swoole_dns_server() {',formatter_signature])+'\n'+gate.body(selected,parser_signature)
    driver=r'''
int main(){
    size_t count=0;
    swr_v1_clients_set_dns_server(reinterpret_cast<const uint8_t *>("127.0.0.1:53"),12);
    std::vector<std::string> domains={"","a","www.apple.com","localhost",".","a.",".a","a..b","a b.c","a:b.c"};
    for(unsigned byte=0;byte<256;byte++)for(unsigned length:{1,2,4,63,127,128,255,256})domains.push_back(std::string(length,char(byte)));
    for(unsigned n=1;n<=127;n++)domains.push_back(std::string(n,'a')+"."+std::string(128-n,'b')+".example");
    for(const auto&domain:domains){
        std::vector<char>a(8192,'!'),b=a;
        int ar=native::domain_encode(domain.c_str(),domain.size(),a.data()),br=rust::domain_encode(domain.c_str(),domain.size(),b.data());
        if(ar!=br||a!=b)return 1;++count;
    }
    std::vector<std::string> labels={"a","www","example","com",std::string(63,'a'),std::string(127,'z')};
    for(const auto&a:labels)for(const auto&b:labels)for(const auto&c:labels){
        auto domain=a+"."+b+"."+c;
        std::vector<char> expected(8192,'!');native::domain_encode(domain.c_str(),domain.size(),expected.data());auto actual=expected;
        const auto initialized=std::strlen(expected.data())+1;
        native::domain_decode(expected.data());rust::domain_decode(actual.data(),initialized);
        if(actual!=expected)return 2;++count;
    }
    // Embedded NUL and initialized padding can have defined native behavior.
    // Compare the entire initialized backing row, not only its first C-string.
    for(unsigned byte=1;byte<=(std::numeric_limits<char>::is_signed?127:255);byte++)for(unsigned offset:{1,2,3}) {
        std::vector<char> expected(8192,0);expected[0]=char(byte);
        for(unsigned i=1;i<=byte;i++)expected[i]='a';
        expected[std::min(byte,offset)]=0;auto actual=expected;
        native::domain_decode(expected.data());rust::domain_decode(actual.data(),actual.size());
        if(actual!=expected)return 4;++count;
    }
    // The old helper explicitly rejects src[n]=='.' even with a shorter n.
    {char a[32]={},b[32]={};if(native::domain_encode("a.",1,a)!=rust::domain_encode("a.",1,b))return 3;++count;}
    // Whole original/patched query/response callers; aligned records preserve
    // the original parser's defined native header alignment. IO is a double.
    for(const auto&domain:std::vector<std::string>{"a.co","abc.test","long.example"})for(int record:{1,28,5})for(double timeout:{0.0,0.1})for(bool send:{false,true})for(int receive:{0,1,2})for(bool cancel:{false,true})for(bool bad:{false,true}){
        record_type=record;send_success=send;receive_mode=receive;canceled=cancel;bad_id=bad;
        last_error=0;io_effects.clear();auto expected=native::dns_lookup_impl_with_socket(domain.c_str(),record==28?AF_INET6:AF_INET,timeout);auto effects=io_effects;auto query=sent_query;int error=last_error;
        last_error=0;io_effects.clear();auto actual=rust::dns_lookup_impl_with_socket(domain.c_str(),record==28?AF_INET6:AF_INET,timeout);
        if(expected!=actual||effects!=io_effects||query!=sent_query||error!=last_error)return 5;++count;
    }
    std::cout<<count<<" "<<std::numeric_limits<char>::is_signed<<"\n";
}
'''
    cpp=out/'actual_codec.cpp';cpp.write_text(prelude+'\nnamespace native {\n'+native+'\n}\nnamespace rust {\n'+rust+'\n}\n'+driver);exe=out/'actual_codec';flags=['-fsanitize=address,undefined','-fno-omit-frame-pointer'] if a.sanitize else []
    gate.run(['c++','-std=c++17','-O1',*flags,'-I',ROOT/'native/clients',cpp,archive,'-lpthread','-ldl','-lm','-o',exe]);values=gate.run([exe]).strip().split();cases=int(values[0]);signed_char=bool(int(values[1]))
    report=dict(passed=True,cases=cases,scope=__doc__,pin=gate.PIN,native_sanitized=a.sanitize,native_char_signed=signed_char,rust_sanitized=a.rust_sanitized,not_compared=['Inputs outside the compared domain run on Rust only; Rust stops at bounds, retaining any preceding in-bounds writes.'])
    a.output.write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report))

if __name__=='__main__':main()
