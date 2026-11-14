// Run every registered byte-identical pinned original, with no added edge groups.
#include "test_core.h"
#include <iostream>
int main() {
    SwooleG.std_allocator={std::malloc,std::calloc,std::realloc,std::free};SwooleG.pagesize=4096;
    unsigned failed=0;
    std::vector<std::pair<const char *,bool>> outcomes;
    for(const auto &test:string_tests()) {
        bool passed=true;
        try { test.second(); }
        catch(const std::exception &error) { passed=false;std::cerr<<test.first<<": "<<error.what()<<'\n'; }
        catch(...) { passed=false;std::cerr<<test.first<<": unknown exception\n"; }
        outcomes.emplace_back(test.first,passed);if(!passed)++failed;
    }
    std::cout<<"{\"original_tests\":"<<outcomes.size()<<",\"failed\":"<<failed<<",\"original_ids\":[";
    for(size_t i=0;i<outcomes.size();++i){if(i)std::cout<<',';std::cout<<'"'<<outcomes[i].first<<'"';}
    std::cout<<"],\"outcomes\":[";
    for(size_t i=0;i<outcomes.size();++i){if(i)std::cout<<',';std::cout<<"{\"id\":\""<<outcomes[i].first<<"\",\"passed\":"<<(outcomes[i].second?"true":"false")<<'}';}
    std::cout<<"],\"passed\":"<<(failed?"false":"true")<<"}\n";
    return failed?1:0;
}
