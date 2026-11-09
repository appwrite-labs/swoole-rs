#include "swoole_rs_timer.h"
#include <algorithm>
#include <climits>
#include <cstdio>
#include <cstdlib>
#include <functional>
#include <map>
#include <string>
#include <vector>
#include <sys/wait.h>
#include <unistd.h>
#include <type_traits>

#define CHECK(x) do { if (!(x)) { std::fprintf(stderr, "%s:%d: %s\n", __FILE__, __LINE__, #x); std::exit(1); } } while (0)
static_assert(std::is_standard_layout<swr_v1_timer_view>::value, "C layout");
static_assert(sizeof(void *) == 8 && sizeof(size_t) == 8, "supported 64-bit hosts");
static_assert(sizeof(swr_v1_timer_view) == 56 && offsetof(swr_v1_timer_view, token) == 48, "view layout");
static_assert(offsetof(swr_v1_timer_view, removed) == 40 && sizeof(swr_v1_timer_stats) == 32, "stats layout");
static_assert(alignof(swr_v1_timer_view) == 8 && alignof(swr_v1_timer_stats) == 8, "alignment");
static_assert(offsetof(swr_v1_timer_view, deadline) == 8 && offsetof(swr_v1_timer_view, interval) == 16, "deadline layout");
static_assert(offsetof(swr_v1_timer_view, exec_count) == 24 && offsetof(swr_v1_timer_view, birth_round) == 32, "counter layout");
static_assert(offsetof(swr_v1_timer_stats, round) == 8 && offsetof(swr_v1_timer_stats, active_id) == 16, "stats counters");
static_assert(offsetof(swr_v1_timer_stats, selecting) == 24 && offsetof(swr_v1_timer_stats, closing) == 28, "stats flags");
static_assert(std::is_same<decltype(swr_v1_timer_view::id), int64_t>::value, "signed ID");
static_assert(std::is_same<decltype(swr_v1_timer_view::exec_count), uint64_t>::value, "execution width");

struct Node { std::string name; int drops = 0; std::function<void()> destroy; };
struct Queue {
    swr_v1_timer *q = nullptr;
    std::vector<std::string> trace;
    std::map<int64_t, Node *> owned;
    Queue() { CHECK(swr_v1_timer_create(&q) == SWR_TIMER_OK && q); }
    ~Queue() { CHECK(owned.empty()); CHECK(swr_v1_timer_release(q) == SWR_TIMER_OK); }
    swr_v1_timer_view add(Node &n, int64_t now, int64_t delay, bool repeat = false) {
        swr_v1_timer_view v{};
        CHECK(swr_v1_timer_add(q, now, delay, repeat, &n, &v) == SWR_TIMER_OK);
        CHECK(v.id > 0 && v.token == &n && v.deadline == now + delay && !v.removed && !v.reserved);
        CHECK(v.interval == (repeat ? delay : 0) && !v.exec_count);
        CHECK(owned.emplace(v.id, &n).second);
        return v;
    }
    swr_v1_timer_stats stats() { swr_v1_timer_stats s{}; CHECK(swr_v1_timer_stats_get(q, &s) == 0); CHECK(s.count == owned.size()); return s; }
    void drop(swr_v1_timer_view v) {
        CHECK(owned.count(v.id) && owned[v.id] == v.token);
        Node *n = owned[v.id]; owned.erase(v.id);
        CHECK(++n->drops == 1); trace.push_back("drop:" + n->name);
        swr_v1_timer_view missing{};
        CHECK(swr_v1_timer_get(q, v.id, &missing) == SWR_TIMER_NOT_FOUND);
        CHECK(!stats().active_id || stats().active_id != v.id);
        if (n->destroy) n->destroy();  // All Rust calls have returned before native destruction.
    }
    void remove(int64_t id) { swr_v1_timer_view v{}; CHECK(swr_v1_timer_remove(q, id, &v) == SWR_TIMER_RELEASE); drop(v); }
    void prime(int64_t now = 0) {
        CHECK(swr_v1_timer_begin(q, now) == 0); swr_v1_timer_view v{};
        CHECK(swr_v1_timer_next(q, &v) == SWR_TIMER_EMPTY); int64_t delay;
        CHECK(swr_v1_timer_end(q, &delay) == 0); CHECK(stats().round == 1);
    }
    void select(int64_t now, std::function<void(swr_v1_timer_view)> callback = {}) {
        CHECK(swr_v1_timer_begin(q, now) == 0);
        while (true) {
            swr_v1_timer_view v{}; int status = swr_v1_timer_next(q, &v);
            if (status == SWR_TIMER_EMPTY) break;
            if (status == SWR_TIMER_CLOSING) { CHECK(stats().closing && !v.id && !v.token); break; }
            CHECK(status == 0 && stats().active_id == v.id);
            trace.push_back("call:" + static_cast<Node *>(v.token)->name);
            if (callback) callback(v);  // Native callback, no Rust borrow retained.
            status = swr_v1_timer_complete(q, v.id, &v);
            if (status == SWR_TIMER_RELEASE_OVERFLOW) trace.push_back("overflow:" + static_cast<Node *>(v.token)->name);
            if (status == SWR_TIMER_RELEASE || status == SWR_TIMER_RELEASE_OVERFLOW) drop(v);
            else CHECK(status == SWR_TIMER_OK && !stats().active_id);
        }
        int64_t delay; CHECK(swr_v1_timer_end(q, &delay) == 0);
    }
    void close() {
        CHECK(swr_v1_timer_close(q) == 0); CHECK(swr_v1_timer_close(q) == 0);
        swr_v1_timer_view v{};
        int status;
        while ((status = swr_v1_timer_drain(q, &v)) == SWR_TIMER_RELEASE) drop(v);
        CHECK(status == SWR_TIMER_EMPTY || status == SWR_TIMER_DEFERRED);
        CHECK(!v.id && !v.token);
    }
    void golden(std::initializer_list<const char *> expected) {
        std::vector<std::string> e; for (auto x : expected) e.emplace_back(x);
        CHECK(trace == e);
    }
};

static void active_cancel() {
    Queue q; Node a{"A", 0, {}}; auto n = q.add(a, 0, 1, true); q.prime();
    q.select(1, [&](auto v) { swr_v1_timer_view out{};
        CHECK(swr_v1_timer_remove(q.q, v.id, &out) == SWR_TIMER_DEFERRED && out.removed);
        CHECK(swr_v1_timer_remove(q.q, v.id, &out) == SWR_TIMER_NOT_FOUND);
        CHECK(swr_v1_timer_get(q.q, v.id, &out) == 0 && out.removed && out.token == &a);
        CHECK(q.stats().count == 1 && !a.drops && n.id == v.id);
    }); q.golden({"call:A", "drop:A"});
}
static void active_delay() {
    Queue q; Node a{"A",0,{}}, b{"B",0,{}}; auto av=q.add(a,0,1); q.add(b,0,2); q.prime();
    q.select(2,[&](auto v) { if(v.id==av.id) { swr_v1_timer_view out{};
        CHECK(swr_v1_timer_update(q.q,v.id,20,&out)==0 && out.deadline==20);
        CHECK(swr_v1_timer_complete(q.q,v.id+100,&out)==SWR_TIMER_INVALID);
    }}); q.golden({"call:A","drop:A","call:B","drop:B"});
    Queue r; Node c{"C",0,{}}; auto cv=r.add(c,0,5,true); r.prime();
    r.select(5,[&](auto v){ swr_v1_timer_view out{}; CHECK(swr_v1_timer_update(r.q,v.id,20,&out)==0); });
    swr_v1_timer_view out{}; CHECK(swr_v1_timer_get(r.q,cv.id,&out)==0 && out.deadline==20 && out.exec_count==1);
    r.close();
}
static void recursive() {
    Queue q; Node a{"A",0,{}}; q.add(a,0,1); q.prime(); q.select(1,[&](auto){
        auto before=q.stats(); CHECK(swr_v1_timer_begin(q.q,1)==SWR_TIMER_BUSY);
        swr_v1_timer_view v{}; CHECK(swr_v1_timer_next(q.q,&v)==SWR_TIMER_BUSY);
        int64_t delay=99; CHECK(swr_v1_timer_end(q.q,&delay)==SWR_TIMER_BUSY && delay==-1);
        CHECK(q.stats().round==before.round);
    }); q.golden({"call:A","drop:A"});
}
static void mutation() {
    Queue q; Node a{"A",0,{}},b{"B",0,{}},c{"C",0,{}};
    q.add(a,0,1); auto bv=q.add(b,0,2); auto cv=q.add(c,0,3);
    b.destroy=[&]{q.remove(cv.id);}; q.prime(); q.select(3,[&](auto){q.remove(bv.id);});
    q.golden({"call:A","drop:B","drop:C","drop:A"});
}
static void ties() {
    Queue q; Node a{"A",0,{}},b{"B",0,{}},c{"C",0,{}};
    q.add(a,0,1);q.add(b,0,1);q.add(c,0,1);q.prime();q.select(1);
    q.golden({"call:A","drop:A","call:C","drop:C","call:B","drop:B"});
}
static void missed() {
    Queue q;Node a{"A",0,{}};auto av=q.add(a,0,5,true);q.prime();q.select(23);
    swr_v1_timer_view v{};CHECK(swr_v1_timer_get(q.q,av.id,&v)==0 && v.deadline==25 && v.exec_count==1);
    q.select(24);CHECK(swr_v1_timer_get(q.q,av.id,&v)==0 && v.exec_count==1);
    q.select(25);CHECK(swr_v1_timer_get(q.q,av.id,&v)==0 && v.deadline==30 && v.exec_count==2);
    q.close();q.golden({"call:A","call:A","drop:A"});
}
static void captured() {
    Queue q;Node a{"A",0,{}},b{"B",0,{}},c{"C",0,{}};
    q.add(a,0,1);q.add(b,0,50);q.prime();
    q.select(1,[&](auto){q.add(c,100,1);});q.golden({"call:A","drop:A"});
    q.select(100);q.golden({"call:A","drop:A","call:B","drop:B"});q.close();
}
static void birth() {
    Queue q;Node a{"A",0,{}},b{"B",0,{}},c{"C",0,{}};
    auto av=q.add(a,0,1);auto bv=q.add(b,0,2);q.prime(2);
    q.select(2,[&](auto v){if(v.id==av.id){ auto cv=q.add(c,2,1);swr_v1_timer_view out{};
        CHECK(swr_v1_timer_update(q.q,cv.id,0,&out)==0); }});
    // Newly born C is heap head and blocks already-due B in this round.
    q.golden({"call:A","drop:A"});CHECK(q.stats().count==2);
    swr_v1_timer_view out{};CHECK(swr_v1_timer_get(q.q,bv.id,&out)==0);
    q.select(3);q.golden({"call:A","drop:A","call:C","drop:C","call:B","drop:B"});
}
static void snapshot() {
    Queue q;Node a{"A",0,{}},b{"B",0,{}},c{"C",0,{}};
    auto av=q.add(a,0,1);auto bv=q.add(b,0,1);a.destroy=[&]{q.add(c,0,1);};
    size_t count=0;CHECK(swr_v1_timer_ids(q.q,nullptr,0,&count)==SWR_TIMER_CAPACITY && count==2);
    int64_t guard[2]={-77,-88};CHECK(swr_v1_timer_ids(q.q,guard,1,&count)==SWR_TIMER_CAPACITY && guard[0]==-77 && guard[1]==-88);
    CHECK(swr_v1_timer_ids(q.q,guard,2,&count)==0);std::sort(guard,guard+2);CHECK(guard[0]==av.id && guard[1]==bv.id);
    q.remove(guard[0]);q.remove(guard[1]);CHECK(q.stats().count==1);q.close();q.golden({"drop:A","drop:B","drop:C"});
}
static void limits() {
    Queue q;Node a{"A",0,{}};swr_v1_timer_view v{};
    CHECK(swr_v1_timer_add(q.q,INT64_MAX-2,3,0,&a,&v)==SWR_TIMER_OVERFLOW && !v.id && !v.token);
    CHECK(swr_v1_timer_add(q.q,0,0,0,&a,&v)==SWR_TIMER_INVALID);
    CHECK(swr_v1_timer_add(q.q,0,1,2,&a,&v)==SWR_TIMER_INVALID);
    CHECK(swr_v1_timer_add(q.q,0,1,0,nullptr,&v)==SWR_TIMER_INVALID);
    auto av=q.add(a,10,1);CHECK(av.id==1);
    CHECK(swr_v1_timer_add(q.q,9,1,0,&a,&v)==SWR_TIMER_INVALID && q.stats().count==1);
    CHECK(swr_v1_timer_begin(q.q,9)==SWR_TIMER_INVALID && !q.stats().selecting);
    CHECK(swr_v1_timer_update(q.q,av.id,-1,&v)==SWR_TIMER_INVALID);
    CHECK(swr_v1_timer_get(q.q,av.id,&v)==0 && v.deadline==11);
    CHECK(swr_v1_timer_release(q.q)==SWR_TIMER_PENDING);q.close();q.golden({"drop:A"});
}
static void overflow() {
    Queue q;Node a{"A",0,{}};q.add(a,INT64_MAX-2,1,true);q.prime(INT64_MAX-2);q.select(INT64_MAX);
    q.golden({"call:A","overflow:A","drop:A"});
}
static void teardown() {
    Queue q;Node a{"A",0,{}},b{"B",0,{}},c{"C",0,{}};q.add(a,0,1);q.add(b,0,2);q.prime();
    q.select(2,[&](auto v){q.close();swr_v1_timer_view out{};
        CHECK(swr_v1_timer_drain(q.q,&out)==SWR_TIMER_DEFERRED && !out.id && !out.token);
        CHECK(swr_v1_timer_add(q.q,2,1,0,&c,&out)==SWR_TIMER_CLOSING);
        CHECK(swr_v1_timer_update(q.q,v.id,3,&out)==SWR_TIMER_CLOSING);
        CHECK(q.stats().count==1 && !a.drops && q.stats().closing);
    });q.golden({"call:A","drop:B","drop:A"});
}
static void fork_copy() {
    Queue q;Node a{"A",0,{}};auto av=q.add(a,0,1);pid_t child=fork();CHECK(child>=0);
    if(child==0){q.close();CHECK(a.drops==1 && q.stats().count==0);CHECK(swr_v1_timer_release(q.q)==0);_exit(0);}
    int status;CHECK(waitpid(child,&status,0)==child && WIFEXITED(status) && WEXITSTATUS(status)==0);
    swr_v1_timer_view v{};CHECK(!a.drops && swr_v1_timer_get(q.q,av.id,&v)==0 && v.token==&a);
    q.prime();q.select(1);q.golden({"call:A","drop:A"});
}
int main(){
    CHECK(swr_v1_timer_create(nullptr)==SWR_TIMER_INVALID);
    swr_v1_timer_view v{};v.id=7;CHECK(swr_v1_timer_get(nullptr,1,&v)==SWR_TIMER_INVALID && !v.id);
    swr_v1_timer_stats invalid{}; invalid.count=7;
    CHECK(swr_v1_timer_stats_get(nullptr,&invalid)==SWR_TIMER_INVALID && !invalid.count);
    { Queue q; CHECK(swr_v1_timer_drain(q.q,&v)==SWR_TIMER_INVALID && !v.id);
      CHECK(swr_v1_timer_next(q.q,&v)==SWR_TIMER_INVALID && !v.id);
      CHECK(swr_v1_timer_complete(q.q,1,&v)==SWR_TIMER_INVALID && !v.id);
      CHECK(swr_v1_timer_get(q.q,0,&v)==SWR_TIMER_INVALID && !v.id);
      CHECK(swr_v1_timer_begin(q.q,0)==0);CHECK(swr_v1_timer_release(q.q)==SWR_TIMER_PENDING);
      int64_t delay=77; CHECK(swr_v1_timer_end(q.q,&delay)==0 && delay==-1);
    }
    active_cancel();active_delay();recursive();mutation();ties();missed();captured();birth();snapshot();limits();overflow();teardown();fork_copy();
    std::puts("{\"topics_passed\":13,\"topics\":[\"active_cancel\",\"active_delay\",\"nested_selection\",\"callback_destructor_mutation\",\"equal_deadlines\",\"missed_intervals\",\"captured_time\",\"birth_round_head_blocking\",\"snapshot_mutation\",\"checked_limits\",\"repeat_deadline_overflow\",\"callback_close_drain\",\"unix_fork_copy\"],\"actual_unix_fork\":true,\"native_tokens_exactly_once\":true}");
}
