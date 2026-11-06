#include <cassert>
#include <cstdint>
#include <cstdlib>
#include <functional>
#include <iostream>
#include <limits>
#include <list>
#include <memory>
#include <utility>
#include <vector>
#include "swoole_rs_reactor.h"
#include "builtins.inc"
using namespace swoole;
int main(){
    unsigned checks=0,cases=0;auto check=[&](bool value){assert(value);++checks;};std::vector<int> trace;
    {
        Reactor r;size_t events=0;check(defer_exit(&r,events));defer_end(&r);check(r.defer_tasks==nullptr);++cases;
        r.defer([&](void *data){check(data==&trace);check(r.defer_tasks==nullptr);trace.push_back(1);},&trace);
        r.defer([&](void*){trace.push_back(2);});check(!defer_exit(&r,events));
        defer_end(&r);check(trace==std::vector<int>({1,2}));check(r.defer_tasks==nullptr);check(defer_exit(&r,events));trace.clear();++cases;
        r.defer([&](void*){check(r.defer_tasks==nullptr);trace.push_back(3);r.defer([&](void*){trace.push_back(4);});});
        defer_end(&r);check(trace==std::vector<int>({3}));check(r.defer_tasks!=nullptr);check(!defer_exit(&r,events));++cases;
        defer_end(&r);check(trace==std::vector<int>({3,4}));check(r.defer_tasks==nullptr);check(defer_exit(&r,events));trace.clear();++cases;
    }
    for(bool present:{false,true}){
        Reactor r;
        if(present){r.future_task.callback=[&](void *data){check(data==&trace);trace.push_back(5);};r.future_task.data=&trace;r.idle_task.callback=[&](void *data){check(data==&trace);trace.push_back(6);};r.idle_task.data=&trace;}
        r.execute_begin_callback();idle_end(&r);check(trace==(present?std::vector<int>({5,6}):std::vector<int>{}));trace.clear();++cases;
    }
    for(bool wait:{false,true})for(bool accepted:{false,true})for(bool mutation:{false,true}){
        Reactor r;r.wait_exit=wait;r.accepted=accepted;r.mutate_wait=mutation;
        try_exit(&r);check(r.queries==unsigned(wait));check(r.running==!(wait&&accepted));check(r.wait_exit==(wait&&!mutation));++cases;
    }
    for(size_t events:{size_t(0),size_t(1),std::numeric_limits<size_t>::max()}){
        Reactor r;check(default_exit(&r,events)==(events==0));++cases;
    }
    {
        Reactor r;r.timeout_msec=73;check(r.get_timeout_msec()==73);check(!r.defer_tasks);
        r.defer([&](void*){check(r.get_timeout_msec()==73);check(!r.defer_tasks);});
        check(r.get_timeout_msec()==0);check(bool(r.defer_tasks));
        defer_end(&r);check(r.get_timeout_msec()==73);check(!r.defer_tasks);
        r.timeout_msec=-1;check(r.get_timeout_msec()==-1);++cases;
    }
    std::cout<<"{\"cases\":"<<cases<<",\"checks\":"<<checks<<",\"ordinary_deferred_scheduling\":true,\"destruction_observers_or_fault_probes\":false}\n";
}
