#include <cassert>
#include <cstdint>
#include <cstddef>
#include <functional>
#include <iostream>
#include <map>
#include <memory>
#include <vector>
#include "swoole_rs_reactor.h"
#include "callbacks.inc"
using swoole::Reactor;
int main() {
    unsigned checks=0;std::vector<int> trace;
    auto check=[&](bool value) { assert(value);++checks; };
    {
        Reactor r;check(r.if_exit());r.execute_end_callbacks(false);check(!r.timed_out);
        check(!r.isset_exit_condition(1));check(r.remove_exit_condition(1)==0);r.erase_end_callback(1);
    }
    {
        Reactor r;
        r.set_end_callback(5,[&](Reactor *p){check(p->timed_out);trace.push_back(5);});
        r.set_end_callback(1,[&](Reactor*){trace.push_back(1);});
        r.set_end_callback(3,[&](Reactor*){trace.push_back(3);});
        r.execute_end_callbacks(true);check(trace==std::vector<int>({1,3,5}));trace.clear();
        r.set_end_callback(3,[&](Reactor*){trace.push_back(99);});
        r.execute_end_callbacks(true);check(trace==std::vector<int>({1,99,5}));trace.clear();
        r.erase_end_callback(3);r.erase_end_callback(7);
        r.execute_end_callbacks(true);check(trace==std::vector<int>({1,5}));trace.clear();
    }
    {
        Reactor r;
        r.set_end_callback(1,[&](Reactor *p){trace.push_back(1);p->set_end_callback(2,[&](Reactor*){trace.push_back(2);});p->set_end_callback(0,[&](Reactor*){trace.push_back(0);});p->erase_end_callback(3);});
        r.set_end_callback(3,[&](Reactor*){trace.push_back(3);});
        r.set_end_callback(5,[&](Reactor*){trace.push_back(5);});
        r.execute_end_callbacks(false);check(trace==std::vector<int>({1,2,5}));trace.clear();check(!r.timed_out);
    }
    {
        Reactor r;
        r.set_exit_condition(3,[&](Reactor*,size_t &n){trace.push_back(3);check(n==12);return true;});
        r.set_exit_condition(1,[&](Reactor*,size_t &n){trace.push_back(1);check(n==5);n+=7;return true;});
        r.set_exit_condition(2,[&](Reactor*,size_t &n){trace.push_back(2);check(n==12);return false;});
        check(r.isset_exit_condition(2));check(!r.if_exit());check(trace==std::vector<int>({1,2}));trace.clear();
        check(r.remove_exit_condition(2)==1);check(r.remove_exit_condition(2)==0);check(!r.isset_exit_condition(2));
        check(r.if_exit());check(trace==std::vector<int>({1,3}));trace.clear();
    }
    {
        Reactor r;
        r.set_exit_condition(1,[&](Reactor *p,size_t &n){trace.push_back(1);p->set_exit_condition(2,[&](Reactor*,size_t &count){trace.push_back(2);check(count==6);return true;});p->remove_exit_condition(3);++n;return true;});
        r.set_exit_condition(3,[&](Reactor*,size_t &){trace.push_back(3);return false;});
        check(r.if_exit());check(trace==std::vector<int>({1,2}));trace.clear();
    }
    {
        Reactor original;
        original.set_end_callback(1,[&](Reactor*){trace.push_back(1);});
        original.set_exit_condition(1,[](Reactor*,size_t &n){return n==5;});
        Reactor copied(original);check(copied.remove_exit_condition(1)==1);check(original.isset_exit_condition(1));check(!copied.isset_exit_condition(1));copied.set_end_callback(1,[&](Reactor*){trace.push_back(2);});
        original.execute_end_callbacks(false);copied.execute_end_callbacks(false);check(trace==std::vector<int>({1,2}));trace.clear();
        copied=original;check(copied.isset_exit_condition(1));check(copied.if_exit());copied.execute_end_callbacks(false);check(trace==std::vector<int>({1}));trace.clear();
        const Reactor *self=&copied;copied=*self;copied.execute_end_callbacks(false);check(trace==std::vector<int>({1}));trace.clear();
    }
    std::cout<<"{\"checks\":"<<checks<<",\"normal_ordered_mutation\":true,\"native_std_function_effects\":true,\"lifetime_or_fault_probes\":false}\n";
}
