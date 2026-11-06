// SPDX-License-Identifier: Apache-2.0
// Invoke actual native entrypoints; no replacement helper bodies.
#include "swoole.h"
#include "swoole_protocol.h"
#include <cstdlib>
#include <iostream>
#include <sstream>
#include <clocale>
#include <stdexcept>
#include <new>
static bool fail_next_large_allocation = false;
void *operator new(std::size_t size){if(fail_next_large_allocation&&size>15){fail_next_large_allocation=false;throw std::bad_alloc();}if(void*p=std::malloc(size?size:1))return p;throw std::bad_alloc();}
void *operator new[](std::size_t size){if(fail_next_large_allocation&&size>15){fail_next_large_allocation=false;throw std::bad_alloc();}if(void*p=std::malloc(size?size:1))return p;throw std::bad_alloc();}
void operator delete(void*p) noexcept {std::free(p);}
void operator delete[](void*p) noexcept {std::free(p);}
void operator delete(void*p,std::size_t) noexcept {std::free(p);}
void operator delete[](void*p,std::size_t) noexcept {std::free(p);}
static std::string decode(const std::string&s){std::string r;for(size_t i=0;i<s.size();i+=2)r.push_back(char(std::stoul(s.substr(i,2),nullptr,16)));return r;}
static std::string encode(const std::string&s){const char*d="0123456789abcdef";std::string r;for(unsigned char c:s){r+=d[c>>4];r+=d[c&15];}return r.empty()?"-":r;}
int main(){SwooleG.std_allocator = {malloc, calloc, realloc, free};std::string line;while(std::getline(std::cin,line)){std::istringstream in(line);std::string id,op,a,b,loc;int initial;in>>id>>op>>loc>>initial>>a>>b;
if(!std::setlocale(LC_ALL,loc.c_str())){std::cout<<id<<"\tUNAVAILABLE\n";continue;}
// All driver parsing precedes errno initialization so it cannot taint observations.
a=a=="-"?"":a;b=b=="-"?"":b;
unsigned long x=0,y=0;if(op=="type"||op=="radix"||op=="gcd"||op=="lcm")x=std::stoul(a);if(op=="radix"||op=="gcd"||op=="lcm")y=std::stoul(b);
long signed_x=0;if(op=="decimal")signed_x=std::stol(a);
std::string s,t;if(op=="hex"||op=="version"||op=="envstoi"||op=="envstoi_allocfail")s=decode(a);if(op=="version")t=decode(b);
errno=initial;std::string result;
if(op=="type"){int v=swoole_type_size(static_cast<char>(x));int e=errno;result=std::to_string(v);errno=e;}
else if(op=="radix"){char*p=swoole_dec2hex(x,int(y));int e=errno;result=encode(p);free(p);errno=e;}
else if(op=="gcd"||op=="lcm"){uint32_t v=op=="gcd"?swoole_common_divisor(x,y):swoole_common_multiple(x,y);int e=errno;result=std::to_string(v);errno=e;}
else if(op=="decimal"){char buf[128];int n=swoole_itoa(buf,signed_x);int e=errno;result=std::to_string(n)+":"+encode(std::string(buf,n))+":"+std::to_string((unsigned char)buf[n]);errno=e;}
else if(op=="hex"){size_t parsed=123;auto v=swoole_hex2dec(s.c_str(),&parsed);int e=errno;result=std::to_string(v)+":"+std::to_string(parsed);errno=e;}
else if(op=="version"){int v=swoole_version_compare(s.c_str(),t.c_str());int e=errno;result=std::to_string(v);errno=e;}
else if(op=="envstoi"||op=="envstoi_allocfail"){
    if (setenv("SWR_M19_TEST", s.c_str(), 1) != 0) return 2;
    errno = initial;
    int observed_errno = initial;
    int value = 0x13579bdf;
    if (op == "envstoi_allocfail") fail_next_large_allocation = true;
    try {
        bool found = swoole_get_env("SWR_M19_TEST", &value);
        observed_errno = errno;
        result = found ? "value:" + std::to_string(value) : "missing:";
    } catch (const std::invalid_argument &error) {
        observed_errno = errno;
        result = "invalid:" + std::string(error.what()) + ":" + std::to_string(value);
    } catch (const std::out_of_range &error) {
        observed_errno = errno;
        result = "range:" + std::string(error.what()) + ":" + std::to_string(value);
    } catch (const std::bad_alloc &) {
        observed_errno = errno;
        const bool consumed = !fail_next_large_allocation;
        fail_next_large_allocation = false;
        result = "bad_alloc:" + std::to_string(consumed) + ":" + std::to_string(value);
    }
    fail_next_large_allocation = false;
    unsetenv("SWR_M19_TEST");
    errno = observed_errno;
}
int final_errno=errno;std::cout<<id<<"\t"<<result<<"\t"<<final_errno<<"\n";
}}
