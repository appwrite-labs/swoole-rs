// Ordinary default-allocator positive public consumers only. No POP, custom
// allocator/callback, failure injection, release or lifetime/destruction probe.
#include "swoole.h"
#include "swoole_string.h"
#include <cstring>
#include <iostream>
#include <stdexcept>
using swoole::String;
int main() {
    SwooleG.std_allocator={std::malloc,std::calloc,std::realloc,std::free};SwooleG.pagesize=4096;
    unsigned checks=0;
    auto check=[&](bool value){if(!value)throw std::runtime_error("Positive compatibility assertion");++checks;};
    String append(8);append.append("abc",3);append.offset=1;append.append("defghijkl",9);
    check(append.length==12);check(append.size==16);check(append.offset==1);check(std::memcmp(append.str,"abcdefghijkl",12)==0);
    String source("XYZ",3);append.append(source);
    check(append.length==15);check(std::memcmp(append.str+12,"XYZ",3)==0);
    String write("abc",3);write.offset=1;write.write(1,"XYZ",3);
    check(write.length==4);check(write.offset==1);check(std::memcmp(write.str,"aXYZ",4)==0);
    String integer(8);integer.append("a",1);integer.append(1234567890);
    check(integer.length==11);check(std::memcmp(integer.str,"a1234567890",11)==0);
    String repeated(8);repeated.repeat("abcdefghijklmnopq",17,3);
    check(repeated.length==51);check(repeated.size==56);
    for(unsigned i=0;i<3;++i)check(std::memcmp(repeated.str+i*17,"abcdefghijklmnopq",17)==0);
    String single(8);single.repeat("Z",1,17);check(single.length==17);check(single.size==24);
    for(unsigned i=0;i<17;++i)check(single.str[i]=='Z');
    source.offset=1;String copied(source);check(copied.length==3);check(copied.offset==1);check(std::memcmp(copied.str,"XYZ",3)==0);
    String terminated(8);terminated.append("abcdefgh",8);terminated.set_null_terminated();
    check(terminated.length==8);check(terminated.str[8]==0);check(terminated.size==16);
    String reduced("abcdef",6);reduced.offset=1;reduced.reduce(2);
    check(reduced.length==4);check(reduced.offset==0);check(std::memcmp(reduced.str,"cdef",4)==0);
    std::cout<<"{\"checks\":"<<checks<<",\"profile\":\"ordinary-no-custom-no-pop\"}\n";
}
