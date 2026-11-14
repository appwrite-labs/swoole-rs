#include "swoole_rs_server_strings.h"
#include <cassert>
#include <algorithm>
using swoole::detail::ServerStrings;
using swoole::detail::ServerMimeTypes;
int main() {
    auto *state=swr_v1_server_state_new();
    ServerStrings locations(state,0), indexes(state,1), types(state,2);
    assert(locations==nullptr);
    locations.insert("/"); locations.insert("/");
    assert(locations && locations.size()==1);
    auto names=std::make_shared<std::vector<std::string>>(std::initializer_list<std::string>{"a","a","b"});
    indexes=names;
    assert(indexes.size()==names->size() && indexes[0]=="a" && indexes[1]=="a" && indexes[2]=="b");
    indexes.emplace_back("b");
    assert(indexes.size()==4);
    assert(std::find(indexes.begin(),indexes.end(),"b")!=indexes.end());
    ServerMimeTypes absent;
    absent=types;
    types.insert("text/html");
    assert(!absent); // A bound null shared_ptr never observes a later replacement.
    ServerMimeTypes first;
    first=types;
    types.insert("image/png");
    assert(first && first->find("image/png")!=first->end());
    auto replacement=std::make_shared<std::unordered_set<std::string>>();
    replacement->insert("text/plain");
    types=replacement;
    assert(first->find("image/png")!=first->end());
    assert(first->find("text/plain")==first->end());
    ServerMimeTypes second;
    second=types;
    assert(second->find("text/plain")!=second->end());
    ServerMimeTypes copy=first;
    copy=second;
    ServerMimeTypes native;
    native=replacement;
    replacement->insert("image/jpeg");
    assert(native->find("image/jpeg")!=native->end());
    swr_v1_server_state_free(state);
    assert(first->find("text/html")!=first->end());
    assert(second->find("text/plain")!=second->end());
    assert(copy->find("text/plain")!=copy->end());
}
