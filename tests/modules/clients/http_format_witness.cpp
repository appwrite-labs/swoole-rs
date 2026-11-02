/* SPDX-License-Identifier: Apache-2.0 */
#include <cerrno>
#include <cstdarg>
#include <cstdio>
#include <cstdint>
#include <cstring>
#include <iostream>
#include <limits>
#include <string>
#include <vector>
#define sw_unlikely(a) (a)
// PINNED_DEFINES
// DECLARATIONS
namespace native {
// NATIVE_PRINTF
// The pinned request formatter calls, by swr_v1_clients_http_format kind.
size_t bounded(unsigned kind,const std::string&a,const std::string&b,const std::string&c,const std::string&d,size_t number,char*output,size_t capacity){
 switch(kind){case 0:return sw_snprintf(output,capacity,"Content-Length: %zu\r\n\r\n",number);case 1:if(std::memchr(a.data(),':',a.size()))return sw_snprintf(output,capacity,"%s%s%s","http://",a.c_str(),b.c_str());else return sw_snprintf(output,capacity,"%s%s:%u%s","http://",a.c_str(),static_cast<unsigned>(number),b.c_str());case 2:return sw_snprintf(output,capacity,"Content-Type: multipart/form-data; boundary=%.*s\r\n",static_cast<int>(a.size()),a.c_str());case 3:return sw_snprintf(output,capacity,SW_HTTP_FORM_RAW_DATA_FMT,static_cast<int>(a.size()),a.c_str(),static_cast<int>(b.size()),b.c_str());case 4:return sw_snprintf(output,capacity,SW_HTTP_FORM_FILE_DATA_FMT,static_cast<int>(a.size()),a.c_str(),static_cast<int>(b.size()),b.c_str(),static_cast<int>(c.size()),c.c_str(),static_cast<int>(d.size()),d.c_str());case 5:return sw_snprintf(output,capacity,"--%.*s--\r\n",static_cast<int>(a.size()),a.c_str());}return 0;
}
}
// ENTRY
#include "swoole_rs_clients.h"
namespace native {
size_t bounded(unsigned kind,const std::string&a,const std::string&b,const std::string&c,const std::string&d,size_t number,char*output,size_t capacity);
}
namespace rust {
// The Rust formatter phpclients calls, with its error mapped to the errno sw_snprintf sets.
size_t bounded(unsigned kind,const std::string&a,const std::string&b,const std::string&c,const std::string&d,size_t number,char*output,size_t capacity){
 auto span=[](const std::string&s){return swr_v1_clients_format_span{reinterpret_cast<const uint8_t*>(s.data()),s.size()};};
 auto result=swr_v1_clients_http_format(kind,span(a),span(b),span(c),span(d),number,reinterpret_cast<uint8_t*>(output),capacity);
 if(result.error)errno=result.error==2?EOVERFLOW:EINVAL;
 return result.written;
}
}
int main(){size_t cases=0;std::vector<size_t> lengths={0,1,7,38,39,40,100,2040,2048,2050};std::vector<size_t> numbers={0,1,9,10,99,100,65535,std::numeric_limits<size_t>::max()};
 for(unsigned byte=0;byte<256;++byte)for(size_t length:lengths){std::string a(39,'b'),b(length,static_cast<char>(byte)),c(length,'f'),d(length,'t');a[byte%39]=static_cast<char>(byte);if(length>1){b[length/2]=0;c[length/2]=0;}if(length>2)b[length-1]=':';
 for(unsigned kind=0;kind<6;++kind)for(size_t capacity:{0,1,2,7,15,16,19,20,21,22,23,39,40,64,2048}){char x[4096],y[4096];std::memset(x,0x5a,sizeof(x));std::memset(y,0x5a,sizeof(y));size_t number=kind==1?byte*257:numbers[byte%numbers.size()];errno=17;auto xn=native::bounded(kind,a,b,c,d,number,x,capacity);int xe=errno;errno=17;auto yn=rust::bounded(kind,a,b,c,d,number,y,capacity);if(xn!=yn||xe!=errno||std::memcmp(x,y,sizeof(x))){std::cerr<<"bounded "<<kind<<":"<<byte<<":"<<length<<":"<<capacity<<":"<<xn<<":"<<yn<<"\n";return 1;}++cases;}}
 for(unsigned kind=0;kind<6;++kind){std::string a="boundary",b="host",c="file",d="application/octet-stream";size_t number=kind==1?65535:std::numeric_limits<size_t>::max();errno=17;auto x=native::bounded(kind,a,b,c,d,number,nullptr,0);int e=errno;errno=17;auto y=rust::bounded(kind,a,b,c,d,number,nullptr,0);if(x!=y||errno!=e)return 1;}
 char defensive[16];auto absent=swr_v1_clients_format_span{nullptr,0};auto bad=swr_v1_clients_format_span{nullptr,1};auto invalid=swr_v1_clients_http_format(2,bad,absent,absent,absent,0,reinterpret_cast<uint8_t*>(defensive),sizeof(defensive));if(invalid.error!=1||invalid.written!=0||defensive[0]!=0)return 1;invalid=swr_v1_clients_http_format(0,absent,absent,absent,absent,0,nullptr,1);if(invalid.error!=1||invalid.written!=0)return 1;
 std::cout<<"{\"bounded_capacity_cases\":"<<cases<<",\"null_output_cases\":6,\"rust_only_invalid_span_cases\":2}\n";}
