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
using ::swoole_str_starts_with;
using ::swoole_strcaseeq;
using ::swoole_streq;
using ::swoole_strncasestr;
using ::swoole_strnpos;
using ::swoole_strnstr;
using ::swoole_strrnpos;
using ::swoole_strtolower;
using swoole::ends_with;
using swoole::ltrim;
using swoole::rtrim;
using swoole::starts_with;
using swoole::substr_len;
}
static ptrdiff_t offset(const char *data, const char *found) { return found ? found-data : -1; }
int main() {
    std::mt19937 random(774812);
    size_t trims=0, searches=0, comparisons=0, partitions=0, locales=0, casefolds=0, lowercase=0;
    std::vector<std::string> available, unavailable;
    std::vector<int> lowercase_i;
    for(const char *locale:{"C", "", "C.UTF-8", "en_US.UTF-8", "en_US.ISO8859-1",
                            "tr_TR.ISO8859-9", "tr_TR.UTF-8"}) {
        if(!setlocale(LC_CTYPE,locale)) { unavailable.emplace_back(locale); continue; }
        available.emplace_back(locale);
        lowercase_i.push_back(tolower('I'));
        locales++;
        std::vector<unsigned> alphabet;
        for (unsigned byte=0; byte<256; ++byte) {
            if (CHAR_MIN >= 0 || byte < 128 || (EOF == -1 && byte == 255)) alphabet.push_back(byte);
        }
        for (size_t trial=0; trial<4096; ++trial) {
            size_t count = trial % 257;
            std::string a(count+32, '#');
            for (size_t i=0; i<count; ++i) a[16+i]=static_cast<char>(alphabet[(i+trial)%alphabet.size()]);
            auto b=a;
            // Negative extent stays inside the actual allocation, so both the
            // original end-pointer expression and its empty loop are defined.
            int length = trial%11==0 ? -8 : static_cast<int>(count);
            errno=EBUSY; original::swoole_strtolower(&a[16],length); int expected_errno=errno;
            errno=EBUSY; candidate::swoole_strtolower(&b[16],length);
            assert(a==b && errno==expected_errno);
            assert(a.substr(0,16)==std::string(16,'#') && a.substr(16+count)==std::string(16,'#'));
            ++lowercase;
        }
        // ISO-8859 Turkish has distinct dotted/dotless I bytes. Compare the
        // actual host libc semantics, including the search's exact-first-byte
        // guard; no ASCII folding or signed-char ctype arguments are assumed.
        for (unsigned a : {unsigned('I'), unsigned('i'), 0xddu, 0xfdu}) {
            for (unsigned b : {unsigned('I'), unsigned('i'), 0xddu, 0xfdu}) {
                const char hay[] = {static_cast<char>(a), 'x', 0};
                const char needle[] = {static_cast<char>(b), 'X', 0};
                assert(original::swoole_strcaseeq(hay, 2, needle, 2) == candidate::swoole_strcaseeq(hay, 2, needle, 2));
                assert(original::swoole_str_istarts_with(hay, 2, needle, 2) == candidate::swoole_str_istarts_with(hay, 2, needle, 2));
                assert(offset(hay, original::swoole_strncasestr(hay, 2, needle, 2)) ==
                       offset(hay, candidate::swoole_strncasestr(hay, 2, needle, 2)));
                casefolds += 3;
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
            assert(offset(hay.data(),original::swoole_strncasestr(hay.data(),hay.size(),needle.data(),needle.size()))==offset(hay.data(),candidate::swoole_strncasestr(hay.data(),hay.size(),needle.data(),needle.size())));searches++;
            assert(original::swoole_streq(hay.data(),hay.size(),needle.data(),needle.size())==candidate::swoole_streq(hay.data(),hay.size(),needle.data(),needle.size()));comparisons++;
            assert(original::swoole_strcaseeq(hay.data(),hay.size(),needle.data(),needle.size())==candidate::swoole_strcaseeq(hay.data(),hay.size(),needle.data(),needle.size()));comparisons++;
            assert(original::swoole_str_starts_with(hay.data(),hay.size(),needle.data(),needle.size())==candidate::swoole_str_starts_with(hay.data(),hay.size(),needle.data(),needle.size()));comparisons++;
            assert(original::swoole_str_istarts_with(hay.data(),hay.size(),needle.data(),needle.size())==candidate::swoole_str_istarts_with(hay.data(),hay.size(),needle.data(),needle.size()));comparisons++;
            assert(original::starts_with(hay.data(),hay.size(),needle.data(),needle.size())==candidate::starts_with(hay.data(),hay.size(),needle.data(),needle.size()));comparisons++;
            assert(original::starts_with(hay,needle)==candidate::starts_with(hay,needle));comparisons++;
            assert(original::ends_with(hay.data(),hay.size(),needle.data(),needle.size())==candidate::ends_with(hay.data(),hay.size(),needle.data(),needle.size()));comparisons++;
            for (bool before:{false,true}) {
                char separator=static_cast<char>(random());
                assert(original::substr_len(hay.data(),hay.size(),separator,before)==candidate::substr_len(hay.data(),hay.size(),separator,before));partitions++;
            }
            for(char &byte:needle) if(byte==0)byte='z';
            if(needle.size()<=hay.size()) {
                assert(original::swoole_strrnpos(hay.data(),needle.c_str(),hay.size())==candidate::swoole_strrnpos(hay.data(),needle.c_str(),hay.size()));searches++;
            }
        }
    }
    assert(candidate::swoole_strnpos("abc",3,"abc",3)==0);
    assert(candidate::swoole_strrnpos("abc","abc",3)==-1);
    assert(candidate::swoole_strrnpos("aaa","a",3)==2);
    assert(candidate::swoole_strncasestr("Abc",3,"abc",3)==nullptr);
    assert(candidate::swoole_strncasestr("aBc",3,"abc",3)!=nullptr);
    for (size_t length:{size_t(0),size_t(1),size_t(3),size_t(64)}) {
        std::string hay(length,'x'),empty;
        assert(candidate::starts_with(hay,empty));
        assert(candidate::ends_with(hay.data(),hay.size(),empty.data(),0));
    }
    const char nul_a[]={'a',0,'X'},nul_b[]={'a',0,'Y'};
    assert(candidate::swoole_streq(nul_a,3,nul_b,3)==1);
    assert(candidate::swoole_str_starts_with(nul_a,3,nul_b,3)==1);
    assert(!candidate::starts_with(nul_a,3,nul_b,3));
    assert(!candidate::ends_with(nul_a,3,nul_b,3));
    // C-string comparison can stop before declared bounds; no full Rust slice.
    const char short_nul[]={0};
    assert(candidate::swoole_streq(short_nul,100,short_nul,100)==1);
    assert(original::swoole_streq(short_nul,100,short_nul,100)==1);
    assert(original::swoole_streq(short_nul,SIZE_MAX,short_nul,SIZE_MAX)==1);
    assert(candidate::swoole_streq(short_nul,SIZE_MAX,short_nul,SIZE_MAX)==1);
    assert(original::swoole_strcaseeq(short_nul,SIZE_MAX,short_nul,SIZE_MAX)==1);
    assert(candidate::swoole_strcaseeq(short_nul,SIZE_MAX,short_nul,SIZE_MAX)==1);
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
             <<",\"locale_casefold_cases\":"<<casefolds<<",\"lowercase_cases\":"<<lowercase<<",\"available_locales\":[";
    for (size_t i=0; i<available.size(); ++i) std::cout<<(i?",":"")<<'"'<<available[i]<<'"';
    std::cout<<"],\"unavailable_locales\":[";
    for (size_t i=0; i<unavailable.size(); ++i) std::cout<<(i?",":"")<<'"'<<unavailable[i]<<'"';
    std::cout<<"],\"native_lowercase_I\":[";
    for (size_t i=0; i<lowercase_i.size(); ++i) std::cout<<(i?",":"")<<lowercase_i[i];
    std::cout<<"],\"errors\":0}\n";
}
