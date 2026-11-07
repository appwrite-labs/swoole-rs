#include <cassert>
#include <cctype>
#include <climits>
#include <clocale>
#include <cerrno>
#include <cstdio>
#include <cstdint>
#include <cstring>
#include <iostream>
#include <random>
#include <string>
#include <vector>
#include <unistd.h>
#include "swoole.h"
#include "swoole_util.h"
namespace original {
#include "original.inc"
}
// The hybrid build's helpers, as its headers declare them.
namespace candidate {
using ::swoole_str_istarts_with;
using ::swoole_strnpos;
using ::swoole_strnstr;
using swoole::ends_with;
using swoole::ltrim;
using swoole::rtrim;
using swoole::starts_with;
using swoole::substr_len;
}
static ptrdiff_t offset(const char *data, const char *found) { return found ? found-data : -1; }
int main() {
    std::mt19937 random(774812);
    size_t trims=0, searches=0, comparisons=0, partitions=0, locales=0, casefolds=0;
    std::vector<std::string> available, unavailable;
    std::vector<int> lowercase_i;
    for(const char *locale:{"C", "", "C.UTF-8", "en_US.UTF-8", "en_US.ISO8859-1",
                            "tr_TR.ISO8859-9", "tr_TR.UTF-8"}) {
        if(!setlocale(LC_CTYPE,locale)) { unavailable.emplace_back(locale); continue; }
        available.emplace_back(locale);
        lowercase_i.push_back(tolower('I'));
        locales++;
        // ISO-8859 Turkish has distinct dotted/dotless I bytes. Compare the
        // actual host libc semantics; no ASCII folding or signed-char ctype arguments are assumed.
        for (unsigned a : {unsigned('I'), unsigned('i'), 0xddu, 0xfdu}) {
            for (unsigned b : {unsigned('I'), unsigned('i'), 0xddu, 0xfdu}) {
                const char hay[] = {static_cast<char>(a), 'x', 0};
                const char needle[] = {static_cast<char>(b), 'X', 0};
                assert(original::swoole_str_istarts_with(hay, 2, needle, 2) == candidate::swoole_str_istarts_with(hay, 2, needle, 2));
                casefolds += 1;
            }
        }
        for(size_t trial=0;trial<12000;trial++) {
            std::string input(trial%129,' ');
            for(char &byte:input) byte=static_cast<char>(random()%(CHAR_MIN<0?128:256));
            for(size_t i=0;i<input.size()/3;i++) input[trial%2?i:input.size()-1-i]=" \t\n\r\v\f"[random()%6];
            auto a=input,b=input;char *pa=a.data(),*pb=b.data();
            auto n=original::ltrim(&pa,a.size()),got=candidate::ltrim(&pb,b.size());
            assert(n==got && pa-a.data()==pb-b.data() && a==b);trims++;
            assert(original::rtrim(static_cast<const char*>(a.data()),a.size())==candidate::rtrim(static_cast<const char*>(b.data()),b.size()));trims++;
            assert(original::rtrim(a.data(),a.size())==candidate::rtrim(b.data(),b.size()) && a==b);trims++;
            std::string hay(trial%139,'a'),needle(1+trial%19,'b');
            for(char &byte:hay) byte=static_cast<char>(random());
            for(char &byte:needle) byte=static_cast<char>(random());
            if(trial%3==0 && needle.size()<=hay.size()) hay.replace(random()%(hay.size()-needle.size()+1),needle.size(),needle);
            assert(offset(hay.data(),original::swoole_strnstr(hay.data(),hay.size(),needle.data(),needle.size()))==offset(hay.data(),candidate::swoole_strnstr(hay.data(),hay.size(),needle.data(),needle.size())));searches++;
            assert(original::swoole_strnpos(hay.data(),hay.size(),needle.data(),needle.size())==candidate::swoole_strnpos(hay.data(),hay.size(),needle.data(),needle.size()));searches++;
            assert(original::swoole_str_istarts_with(hay.data(),hay.size(),needle.data(),needle.size())==candidate::swoole_str_istarts_with(hay.data(),hay.size(),needle.data(),needle.size()));comparisons++;
            assert(original::starts_with(hay.data(),hay.size(),needle.data(),needle.size())==candidate::starts_with(hay.data(),hay.size(),needle.data(),needle.size()));comparisons++;
            assert(original::starts_with(hay,needle)==candidate::starts_with(hay,needle));comparisons++;
            assert(original::ends_with(hay.data(),hay.size(),needle.data(),needle.size())==candidate::ends_with(hay.data(),hay.size(),needle.data(),needle.size()));comparisons++;
            for (bool before:{false,true}) {
                char separator=static_cast<char>(random());
                assert(original::substr_len(hay.data(),hay.size(),separator,before)==candidate::substr_len(hay.data(),hay.size(),separator,before));partitions++;
            }
        }
    }
    assert(candidate::swoole_strnpos("abc",3,"abc",3)==0);
    for (size_t length:{size_t(0),size_t(1),size_t(3),size_t(64)}) {
        std::string hay(length,'x'),empty;
        assert(candidate::starts_with(hay,empty));
        assert(candidate::ends_with(hay.data(),hay.size(),empty.data(),0));
    }
    const char nul_a[]={'a',0,'X'},nul_b[]={'a',0,'Y'};
    assert(!candidate::starts_with(nul_a,3,nul_b,3));
    assert(!candidate::ends_with(nul_a,3,nul_b,3));
    // Zero length never dereferences the cursor slot or mutable range.
    assert(original::ltrim(nullptr, 0) == candidate::ltrim(nullptr, 0));
    assert(original::rtrim(static_cast<char *>(nullptr), 0) ==
           candidate::rtrim(static_cast<char *>(nullptr), 0));
    char *empty_cursor = nullptr;
    assert(candidate::ltrim(&empty_cursor, 0) == 0 && empty_cursor == nullptr);
    // Cursor publication, suffix writes and errno at ordinary per-byte inputs.
    for (const std::string &value : {std::string(""), std::string("   "),
         std::string("\t\n x \r"), std::string(" \0 ", 3), std::string("x")}) {
        auto a = value, b = value;
        char *pa = a.data(), *pb = b.data();
        errno = EDOM; const auto left = original::ltrim(&pa, a.size()); const int left_errno = errno;
        errno = EDOM; const auto got_left = candidate::ltrim(&pb, b.size());
        assert(left == got_left && errno == left_errno && pa - a.data() == pb - b.data() && a == b);
        errno = EDOM; const auto right = original::rtrim(a.data(), a.size()); const int right_errno = errno;
        errno = EDOM; const auto got_right = candidate::rtrim(b.data(), b.size());
        assert(right == got_right && errno == right_errno && a == b);
    }
    std::cout<<"{\"trim_cases\":"<<trims<<",\"search_cases\":"<<searches<<",\"comparison_cases\":"<<comparisons<<",\"partition_cases\":"<<partitions<<",\"locales\":"<<locales
             <<",\"locale_casefold_cases\":"<<casefolds<<",\"available_locales\":[";
    for (size_t i=0; i<available.size(); ++i) std::cout<<(i?",":"")<<'"'<<available[i]<<'"';
    std::cout<<"],\"unavailable_locales\":[";
    for (size_t i=0; i<unavailable.size(); ++i) std::cout<<(i?",":"")<<'"'<<unavailable[i]<<'"';
    std::cout<<"],\"native_lowercase_I\":[";
    for (size_t i=0; i<lowercase_i.size(); ++i) std::cout<<(i?",":"")<<lowercase_i[i];
    std::cout<<"],\"errors\":0}\n";
}
