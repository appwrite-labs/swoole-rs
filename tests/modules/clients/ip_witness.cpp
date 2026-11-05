/* SPDX-License-Identifier: Apache-2.0 */
#include "swoole_rs_clients.h"
#include <arpa/inet.h>
#include <sys/un.h>
#include <cerrno>
#include <cstdint>
#include <cstring>
#include <iostream>
#include <sstream>
#include <string>
#include <vector>
#define SW_ERROR_BAD_PORT 723
#define SW_ERROR_BAD_HOST_ADDR 722
#define SW_ERROR_DNSLOOKUP_RESOLVE_FAILED 711
#define SW_ERROR_NAME_TOO_LONG 504
#define SW_ERROR_BAD_SOCKET_TYPE 724
// DECLARATIONS
namespace BODY_NAMESPACE {
using SocketType=int;
static thread_local char tmp_address[INET6_ADDRSTRLEN];
static int last_error,dns_calls;static std::string dns_host;
static void swoole_set_last_error(int code){last_error=code;}
static int gethostbyname(int family,const char*host,char*out){++dns_calls;dns_host=host;errno=31;if(dns_host=="resolve-fail")return -1;std::memset(out,family==AF_INET?0x41:0x61,family==AF_INET?4:16);return 0;}
static size_t swoole_strlcpy(char*dst,const char*src,size_t size){size_t length=std::strlen(src);if(size){size_t copy=length<size-1?length:size-1;std::memcpy(dst,src,copy);dst[copy]=0;}return length;}
struct Socket{static bool is_inet4(int t){return t==1||t==2||t==7;}static bool is_inet6(int t){return t==3||t==4||t==8;}static bool is_local(int t){return t==5||t==6;}};
struct Address {
 union {sockaddr_in inet_v4;sockaddr_in6 inet_v6;sockaddr_un un;}addr;
 socklen_t len=0;int type=0;
 static bool verify_ip(int,const std::string&);static bool verify_port(const int,const bool=false);
 bool assign(SocketType,const std::string&,int,bool);bool is_loopback_addr()const;
};
// ACTUAL_BODY
static std::string verify(int family,const std::string&host){std::memset(tmp_address,0x53,sizeof(tmp_address));errno=17;bool result=Address::verify_ip(family,host);std::ostringstream out;out<<result<<':'<<errno<<':';for(unsigned char c:tmp_address)out<<std::hex<<unsigned(c)<<'.';return out.str();}
static std::string assign(int type,const std::string&host,int port,bool resolve){Address address;std::memset(&address.addr,0x73,sizeof(address.addr));address.type=-1;address.len=987;errno=17;last_error=11;dns_calls=0;dns_host.clear();bool result=address.assign(type,host,port,resolve);std::ostringstream out;out<<result<<':'<<errno<<':'<<last_error<<':'<<dns_calls<<':'<<dns_host<<':'<<address.type<<':'<<address.len<<':'<<address.is_loopback_addr()<<':';for(unsigned char c:std::string(reinterpret_cast<char*>(&address.addr),sizeof(address.addr)))out<<std::hex<<unsigned(c)<<'.';return out.str();}
static bool loopback(int type,const std::vector<uint8_t>&bytes){Address address{};address.type=type;if(type==1||type==2)std::memcpy(&address.addr.inet_v4.sin_addr,bytes.data(),4);else std::memcpy(&address.addr.inet_v6.sin6_addr,bytes.data(),16);return address.is_loopback_addr();}
}
// DRIVER
