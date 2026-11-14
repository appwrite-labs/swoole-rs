#include "swoole_http.h"
#include "swoole_rs_http.h"
#include <clocale>
#include <cctype>
#include <iostream>
#include <vector>
namespace upstream_locale {
int get_method(const char *, size_t);
bool parse_multipart_boundary(const char *,size_t,size_t,char **,int *);
// The pinned swoole_strcaseeq and swoole_str_istarts_with; the hybrid headers no longer carry them.
unsigned strcaseeq(const char *,size_t,const char *,size_t);
unsigned istarts_with(const char *,size_t,const char *,size_t);
}
int main() {
 size_t total=0,errors=0,available=0;
 std::cout<<"{\"locales\":[";bool first=true;
 for(const char *locale:{"C","C.UTF-8","en_US.ISO8859-1","tr_TR.ISO8859-9","tr_TR.UTF-8"}) {
  if(!first)std::cout<<',';first=false;
  std::cout<<"{\"locale\":\""<<locale<<"\",\"available\":";
  if(!std::setlocale(LC_ALL,locale)){std::cout<<"false,\"status\":\"unsupported\"}";continue;}
  ++available;size_t cases=0,mismatches=0;std::vector<std::string>witnesses;
  auto compare=[&](bool same,const std::string &witness){++cases;if(!same){++mismatches;if(witnesses.size()<8)witnesses.push_back(witness);}};
  // Actual pinned inline location helper, including every high-byte pair and NUL.
  for(unsigned a=0;a<256;++a)for(unsigned b=0;b<256;++b){
   std::string url="/"+std::string(1,char(a))+"/file",location="/"+std::string(1,char(b));
   bool old=upstream_locale::istarts_with(url.data(),url.size(),location.data(),location.size());
   swr_v1_http_static_path_state state{1,0,0,0};
   int now=swr_v1_http_static_location(reinterpret_cast<const uint8_t *>(url.data()),url.size(),reinterpret_cast<const uint8_t *>(location.data()),location.size(),&state);
   compare(now==int(old)&&state.last==unsigned(old),"location:"+std::to_string(a)+":"+std::to_string(b));
  }
  for(int method=1;method<=26;++method){
   const char *name=swr_v1_http_method_string(method);std::string value=name;
   for(unsigned mask=0;mask<256;++mask){
    value=name;for(size_t i=0;i<value.size();++i)if(mask&(1u<<(i%8)))value[i]=char(std::tolower(static_cast<unsigned char>(value[i])));
    int old=upstream_locale::get_method(value.data(),value.size());
    compare(old==swr_v1_http_method(reinterpret_cast<const uint8_t *>(value.data()),value.size()),"method:"+std::to_string(method)+":"+std::to_string(mask));
   }
  }
  for(unsigned mask=0;mask<256;++mask){
   std::string key="filename";for(size_t i=0;i<key.size();++i)if(mask&(1u<<i))key[i]=char(std::toupper(static_cast<unsigned char>(key[i])));
   bool expected=upstream_locale::strcaseeq(key.data(),key.size(),"filename",8);
   std::string scratch=key+"=x";scratch.push_back(0);
   compare(swr_v1_http_multipart_filename(reinterpret_cast<uint8_t *>(&scratch[0]),scratch.size())==int(expected),"filename:"+std::to_string(mask));
  }
  for(const char *name:{"boundary=","content-disposition"})for(size_t i=0;i<strlen(name);++i)for(unsigned byte=0;byte<256;++byte){
   std::string value=name;value[i]=char(byte);
   if(name[0]=='b'){
    value+="payload";char *old=nullptr;int length=-1;swr_v1_http_span span{};
    bool found=upstream_locale::parse_multipart_boundary(value.data(),value.size(),0,&old,&length);
    int status=swr_v1_http_boundary(reinterpret_cast<const uint8_t *>(value.data()),value.size(),0,&span);
    compare(status==int(found)&&(!found||(span.offset==size_t(old-value.data())&&span.length==size_t(length))),"boundary:"+std::to_string(i)+":"+std::to_string(byte));
   }else{
    bool expected=upstream_locale::strcaseeq(value.data(),value.size(),name,strlen(name));
    compare(swr_v1_http_multipart_disposition(reinterpret_cast<const uint8_t *>(value.data()),value.size())==int(expected),"disposition:"+std::to_string(i)+":"+std::to_string(byte));
   }
  }
  total+=cases;errors+=mismatches;
  std::cout<<"true,\"status\":\""<<(mismatches?"failed":"passed")<<"\",\"cases\":"<<cases<<",\"mismatches\":"<<mismatches<<",\"witnesses\":[";
  for(size_t i=0;i<witnesses.size();++i)std::cout<<(i?",":"")<<'"'<<witnesses[i]<<'"';
  std::cout<<"]}";
 }
 std::cout<<"],\"cases\":"<<total<<",\"available_locales\":"<<available<<",\"errors\":"<<errors<<"}\n";
 return errors||!available?1:0;
}
