// Separately named supported consumer; broad failed profile remains stopped.
// Genuine production Reactor/Socket calls. No embedded backend implementation.
extern "C" {
#include <php.h>
#include <Zend/zend_exceptions.h>
}
#include "swoole.h"
#include "swoole_socket.h"
#include "swoole_reactor.h"
#include <dlfcn.h>
#include <stdexcept>
#include <type_traits>
#include <string>
#include <vector>
#include <sys/socket.h>
#include <unistd.h>
#include <fcntl.h>
using namespace swoole;
namespace {
struct Witness { unsigned reads=0,writes=0;std::string order;bool valid=true; };
int on_read(Reactor *reactor,Event *event) {
    auto *state=static_cast<Witness*>(reactor->ptr);char byte=0;
    state->valid=state->valid&&::recv(event->fd,&byte,1,MSG_DONTWAIT)==1&&byte=='r';
    ++state->reads;state->order+='R';return SW_OK;
}
int on_write(Reactor *reactor,Event *event) {
    auto *state=static_cast<Witness*>(reactor->ptr);
    state->valid=state->valid&&::send(event->fd,"w",1,0)==1;
    ++state->writes;state->order+='W';return SW_OK;
}
void image(zval *result,const char *key,const char *symbol) {
    Dl_info info{};void *address=dlsym(RTLD_DEFAULT,symbol);
    if(address&&dladdr(address,&info))add_assoc_string(result,key,const_cast<char*>(info.dli_fname));
    else add_assoc_null(result,key);
}
}
ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_swr_reactor_supported,0,0,IS_ARRAY,0)
ZEND_END_ARG_INFO()
PHP_FUNCTION(swr_reactor_supported) {
    ZEND_PARSE_PARAMETERS_NONE();array_init(return_value);
    image(return_value,"method_image","_ZN6swoole7ReactorC1EiNS0_4TypeE");
    image(return_value,"storage_image","swr_v1_reactor_poll_new");
    image(return_value,"batch_image","swr_v1_reactor_run");
    image(return_value,"state_image","swr_v1_reactor_state_new");
    image(return_value,"backend_owner_image","swr_v1_reactor_state_open");
    image(return_value,"kernel_wait_image","swr_v1_reactor_state_wait");
    image(return_value,"registry_owner_image","swr_v1_reactor_state_socket");
    zval owner_rows;array_init(&owner_rows);
    auto owner_check=[&](bool value) { add_next_index_bool(&owner_rows,value); };
    zval rows;array_init(&rows);unsigned cases=0;
    auto check=[&](bool value) { add_next_index_bool(&rows,value); };
    try {
        for(Reactor::Type backend:{Reactor::TYPE_POLL,
#ifdef __linux__
                                  Reactor::TYPE_EPOLL
#else
                                  Reactor::TYPE_KQUEUE
#endif
        })for(int mode:{0,1,2,3})for(bool once_socket:{false,true}) {
            // Separate supported inventory: one kqueue filter per ready record.
            if(backend==Reactor::TYPE_KQUEUE&&mode==2)continue;
            Reactor reactor(8,backend);check(reactor.ready_);reactor.once=true;reactor.timeout_msec=20;
            Witness state;reactor.ptr=&state;
            auto timeout_snapshot=reactor.timeout_msec;auto *timeout_slot=&reactor.timeout_msec;
            *timeout_slot=21;owner_check(timeout_snapshot==20&&reactor.timeout_msec==21);reactor.timeout_msec=20;
            auto write_snapshot=reactor.write;reactor.write=nullptr;
            owner_check(write_snapshot!=nullptr&&reactor.write==nullptr);reactor.write=write_snapshot;
            auto once_snapshot=reactor.once;reactor.once=false;
            owner_check(once_snapshot&&!reactor.once);reactor.once=true;
            auto *ptr_slot=&reactor.ptr;owner_check(*ptr_slot==&state&&&reactor.ptr==ptr_slot);
            reactor.set_handler(SW_FD_USER,SW_EVENT_READ,on_read);
            reactor.set_handler(SW_FD_USER,SW_EVENT_WRITE,on_write);
            int pair[2];if(::socketpair(AF_UNIX,SOCK_STREAM,0,pair)!=0)throw std::runtime_error("private socketpair failed");
            auto *socket=make_socket(pair[0],SW_FD_USER);check(socket&&socket->fd==pair[0]);
            const bool readable=mode==0||mode==2,writable=mode==1||mode==2;
            int interest=(readable||mode==3?SW_EVENT_READ:0)|(writable?SW_EVENT_WRITE:0)|(once_socket?SW_EVENT_ONCE:0);
            check(reactor.add(socket,SW_EVENT_READ|SW_EVENT_WRITE)==SW_OK&&reactor.get_event_num()==1);
            check(reactor.set(socket,interest)==SW_OK);
            if(readable&&::send(pair[1],"r",1,0)!=1)throw std::runtime_error("private positive write failed");
            check(reactor.wait()==SW_OK);
            check(state.reads==unsigned(readable));check(state.writes==unsigned(writable));
            check(state.order==(mode==0?"R":mode==1?"W":mode==2?"RW":""));
            check(state.valid);
            const bool removed=once_socket&&mode!=3;
            check(bool(socket->removed)==removed);check(reactor.get_event_num()==unsigned(!removed));
            check(reactor.timed_out==(mode==3));
            if(writable) {char byte=0;check(::recv(pair[1],&byte,1,MSG_DONTWAIT)==1&&byte=='w');}
            else check(true);
            if(!socket->removed)check(reactor.del(socket)==SW_OK);else check(true);
            check(reactor.get_event_num()==0);
            // Token stays live until the complete batch and explicit unregister.
            socket->free();::close(pair[1]);++cases;
        }
        add_assoc_zval(return_value,"owner_outcomes",&owner_rows);
        add_assoc_long(return_value,"cases",cases);add_assoc_zval(return_value,"outcomes",&rows);
    } catch(const std::exception &error) {zval_ptr_dtor(&rows);zend_throw_exception(zend_ce_exception,error.what(),0);}
}
PHP_FUNCTION(swr_reactor_callbacks_supported) {
    ZEND_PARSE_PARAMETERS_NONE();array_init(return_value);
    image(return_value,"method_image","_ZN6swoole7ReactorC1EiNS0_4TypeE");
    image(return_value,"callbacks_image","swr_v1_reactor_callbacks_ensure");
    zval rows;array_init(&rows);
    auto check=[&](bool value){add_next_index_bool(&rows,value);};
    try {
        // Genuine live production Reactor, never copied. Remove only its local
        // installed functions before manual ordinary callback execution.
        Reactor reactor(8,Reactor::TYPE_POLL);check(reactor.ready_);check(reactor.get_event_num()==0);
        for(int key=0;key<=7;++key)reactor.erase_end_callback(static_cast<Reactor::EndCallback>(key));
        check(reactor.remove_exit_condition(Reactor::EXIT_CONDITION_DEFER_TASK)==1);
        check(reactor.remove_exit_condition(Reactor::EXIT_CONDITION_DEFAULT)==1);
        check(!reactor.isset_exit_condition(Reactor::EXIT_CONDITION_TIMER));
        std::vector<int> trace;
        auto end=[&](int key,const std::function<void(Reactor*)>&fn){reactor.set_end_callback(static_cast<Reactor::EndCallback>(key),fn);};
        auto exit=[&](int key,const std::function<bool(Reactor*,size_t&)>&fn){reactor.set_exit_condition(static_cast<Reactor::ExitCondition>(key),fn);};
        end(1,[&](Reactor*){trace.push_back(1);});
        end(3,[&](Reactor*){trace.push_back(3);});
        end(5,[&](Reactor*){trace.push_back(5);});
        reactor.execute_end_callbacks(true);check(reactor.timed_out);check(trace==std::vector<int>({1,3,5}));trace.clear();
        end(3,[&](Reactor*){trace.push_back(99);});
        reactor.execute_end_callbacks(false);check(!reactor.timed_out);check(trace==std::vector<int>({1,99,5}));trace.clear();
        reactor.erase_end_callback(Reactor::PRIORITY_SIGNAL_CALLBACK);reactor.erase_end_callback(Reactor::PRIORITY_TIMER);
        reactor.execute_end_callbacks(false);check(trace==std::vector<int>({1,5}));trace.clear();
        end(3,[&](Reactor*){trace.push_back(3);});
        end(1,[&](Reactor*){
            trace.push_back(1);
            end(0,[&](Reactor*){trace.push_back(0);});
            end(2,[&](Reactor*){trace.push_back(2);});
            reactor.erase_end_callback(Reactor::PRIORITY_SIGNAL_CALLBACK);
        });
        reactor.execute_end_callbacks(false);check(trace==std::vector<int>({1,2,5}));trace.clear();
        exit(0,[&](Reactor*,size_t &events){check(events==0);events+=7;trace.push_back(0);return true;});
        exit(2,[&](Reactor*,size_t &events){check(events==7);trace.push_back(2);return false;});
        exit(3,[&](Reactor*,size_t &events){check(events==7);trace.push_back(3);return true;});
        check(reactor.isset_exit_condition(Reactor::EXIT_CONDITION_WAIT_PID));
        check(!reactor.if_exit());check(trace==std::vector<int>({0,2}));trace.clear();
        check(reactor.remove_exit_condition(Reactor::EXIT_CONDITION_WAIT_PID)==1);
        check(reactor.remove_exit_condition(Reactor::EXIT_CONDITION_WAIT_PID)==0);
        check(reactor.if_exit());check(trace==std::vector<int>({0,3}));
        // All captures stay live and no custom destruction observer is used.
        add_assoc_zval(return_value,"outcomes",&rows);
    } catch(const std::exception &error) {zval_ptr_dtor(&rows);zend_throw_exception(zend_ce_exception,error.what(),0);}
}
PHP_FUNCTION(swr_reactor_builtins_supported) {
    ZEND_PARSE_PARAMETERS_NONE();array_init(return_value);
    image(return_value,"method_image","_ZN6swoole7ReactorC1EiNS0_4TypeE");
    image(return_value,"begin_image","_ZNK6swoole7Reactor22execute_begin_callbackEv");
    image(return_value,"end_image","_ZN6swoole7Reactor21execute_end_callbacksEb");
    image(return_value,"builtins_image","swr_v1_reactor_builtin_step");
#if defined(__APPLE__)
    image(return_value,"defer_method_image","_ZN6swoole7Reactor5deferERKNSt3__18functionIFvPvEEES3_");
#else
    image(return_value,"defer_method_image","_ZN6swoole7Reactor5deferERKSt8functionIFvPvEES2_");
#endif
#if defined(SWOOLE_RS_REACTOR) && SWOOLE_RS_REACTOR
    static_assert(std::is_same<decltype(Reactor::defer_tasks),ReactorDeferredQueue *>::value,"selected deferred field source type");
    static_assert(std::is_same<decltype(Reactor::destroy_callbacks),ReactorDeferredQueue>::value,"selected actual destroy queue source type");
    add_assoc_long(return_value,"defer_owned_compiled",1);
    image(return_value,"defer_push_image","swr_v1_reactor_deferred_push");
    image(return_value,"defer_step_image","swr_v1_reactor_deferred_step");
    image(return_value,"defer_clear_image","swr_v1_reactor_deferred_clear");
#else
    static_assert(std::is_same<decltype(Reactor::defer_tasks),CallbackManager *>::value,"Native deferred field source type");
    static_assert(std::is_same<decltype(Reactor::destroy_callbacks),CallbackManager>::value,"Native actual destroy queue source type");
    add_assoc_long(return_value,"defer_owned_compiled",0);
#endif
    zval rows;array_init(&rows);auto check=[&](bool value){add_next_index_bool(&rows,value);};
    try {
        std::vector<int> trace;bool accepted=false;unsigned queries=0;
        Reactor r(8,Reactor::TYPE_POLL);check(r.ready_);check(r.get_event_num()==0);
        // Keep the three real installed builtin functions; isolate unrelated
        // signal/timer/trim/worker effects from this separately named inventory.
        for(int key=0;key<=7;++key)if(key!=Reactor::PRIORITY_DEFER_TASK&&key!=Reactor::PRIORITY_IDLE_TASK&&key!=Reactor::PRIORITY_TRY_EXIT)r.erase_end_callback(static_cast<Reactor::EndCallback>(key));
        for(int key=0;key<=Reactor::EXIT_CONDITION_IOURING;++key)if(key!=Reactor::EXIT_CONDITION_DEFER_TASK)r.remove_exit_condition(static_cast<Reactor::ExitCondition>(key));
        r.remove_exit_condition(Reactor::EXIT_CONDITION_USER_AFTER_DEFAULT);
        r.running=true;r.wait_exit=false;check(r.defer_tasks==nullptr);check(r.if_exit());
        r.future_task.callback=[&](void *data){check(data==&trace);trace.push_back(0);};r.future_task.data=&trace;
        r.execute_begin_callback();check(trace==std::vector<int>({0}));trace.clear();r.future_task={};
        r.execute_begin_callback();check(trace.empty());
        r.defer([&](void *data){check(data==&trace);check(r.defer_tasks==nullptr);trace.push_back(2);},&trace);
        r.defer([&](void*){trace.push_back(3);});check(!r.if_exit());check(r.defer_tasks!=nullptr);
        r.idle_task.callback=[&](void *data){check(data==&trace);trace.push_back(4);};r.idle_task.data=&trace;
        r.execute_end_callbacks(false);check(trace==std::vector<int>({2,3,4}));check(r.defer_tasks==nullptr);check(r.if_exit());trace.clear();r.idle_task={};
        r.defer([&](void*){check(r.defer_tasks==nullptr);trace.push_back(5);r.defer([&](void*){trace.push_back(6);});});
        r.execute_end_callbacks(false);check(trace==std::vector<int>({5}));check(r.defer_tasks!=nullptr);check(!r.if_exit());
        r.execute_end_callbacks(false);check(trace==std::vector<int>({5,6}));check(r.defer_tasks==nullptr);check(r.if_exit());
        r.set_exit_condition(Reactor::EXIT_CONDITION_USER_BEFORE_DEFAULT,[&](Reactor *self,size_t&){++queries;self->wait_exit=false;return accepted;});
        for(bool value:{false,true}){accepted=value;r.running=true;r.wait_exit=true;unsigned before=queries;r.execute_end_callbacks(false);check(queries==before+1);check(r.running==!accepted);check(!r.wait_exit);}
        check(r.remove_exit_condition(Reactor::EXIT_CONDITION_USER_BEFORE_DEFAULT)==1);
        r.running=true;r.wait_exit=false;r.execute_end_callbacks(false);check(r.running);check(queries==2);
        add_assoc_zval(return_value,"outcomes",&rows);
    } catch(const std::exception &error){zval_ptr_dtor(&rows);zend_throw_exception(zend_ce_exception,error.what(),0);}
}

namespace {
struct OrdinaryOutputPair {
    network::Socket *socket=nullptr;int peer=-1;
    OrdinaryOutputPair(){int pair[2];if(::socketpair(AF_UNIX,SOCK_STREAM,0,pair)!=0)throw std::runtime_error("private output socketpair failed");socket=make_socket(pair[0],SW_FD_USER);peer=pair[1];timeval limit{1,0};if(::setsockopt(peer,SOL_SOCKET,SO_RCVTIMEO,&limit,sizeof(limit))!=0){socket->free();socket=nullptr;::close(peer);peer=-1;throw std::runtime_error("private output receive setup failed");}}
    ~OrdinaryOutputPair(){if(socket)socket->free();if(peer!=-1)::close(peer);}
    std::string receive(size_t length){std::string bytes(length,char(0));size_t offset=0;while(offset<length){auto n=::recv(peer,&bytes[offset],length-offset,0);if(n<=0)throw std::runtime_error("positive output receive failed");offset+=n;}return bytes;}
};
struct OrdinaryOutputFile {
    char path[64]="/tmp/swr-reactor-output-XXXXXX";
    explicit OrdinaryOutputFile(const std::string &bytes){int fd=::mkstemp(path);if(fd<0)throw std::runtime_error("private output file setup failed");auto n=::write(fd,bytes.data(),bytes.size());::close(fd);if(n!=static_cast<ssize_t>(bytes.size())){::unlink(path);throw std::runtime_error("private positive output file write failed");}}
    ~OrdinaryOutputFile(){::unlink(path);}
};
}
PHP_FUNCTION(swr_reactor_output_supported) {
    ZEND_PARSE_PARAMETERS_NONE();array_init(return_value);
    image(return_value,"method_image","_ZN6swoole7network6Socket11handle_sendEv");
    image(return_value,"sendfile_image","_ZN6swoole7network6Socket15handle_sendfileEv");
#if defined(__APPLE__)
    image(return_value,"request_image","_ZN6swoole7network6Socket14sendfile_asyncEPKcxm");
#else
    image(return_value,"request_image","_ZN6swoole7network6Socket14sendfile_asyncEPKclm");
#endif
    image(return_value,"send_policy_image","swr_v1_socket_handle_send");
    image(return_value,"sendfile_cork_image","swr_v1_socket_sendfile_cork");
    image(return_value,"sendfile_sent_image","swr_v1_socket_sendfile_sent");
    image(return_value,"request_policy_image","swr_v1_socket_sendfile_async");
    image(return_value,"request_owner_image","swr_v1_socket_file_new");
    image(return_value,"request_range_image","swr_v1_socket_file_check");
    image(return_value,"request_sent_image","swr_v1_socket_file_sent");
    image(return_value,"request_free_image","swr_v1_socket_file_free");
    image(return_value,"buffer_constructor_image","_ZN6swoole6BufferC1Ej");
#if defined(SWOOLE_RS_BUFFER_TYPED) && SWOOLE_RS_BUFFER_TYPED
    add_assoc_long(return_value,"typed_buffer_compiled",1);
    image(return_value,"typed_buffer_front_image","_ZNK6swoole6Buffer11typed_frontEv");
    image(return_value,"typed_buffer_create_image","swr_v1_buffer_typed_create");
    image(return_value,"typed_buffer_push_image","swr_v1_buffer_typed_push");
    image(return_value,"typed_buffer_front_image_rust","swr_v1_buffer_typed_front");
    image(return_value,"typed_buffer_consume_image","swr_v1_buffer_typed_consume");
    image(return_value,"typed_buffer_pop_image","swr_v1_buffer_typed_pop");
#else
    add_assoc_long(return_value,"typed_buffer_compiled",0);
#endif
    zval rows;array_init(&rows);auto check=[&](bool value){add_next_index_bool(&rows,value);if(!value)throw std::runtime_error("ordinary output assertion failed");};unsigned cases=0;
    try {
        std::string payload;for(unsigned i=0;i<47;++i)payload+=static_cast<char>(i);
        for(size_t length:{size_t(1),size_t(7),size_t(31)}){
            OrdinaryOutputPair pair;auto *socket=pair.socket;check(socket&&socket->fd>=0);
            socket->out_buffer=new Buffer(SW_SEND_BUFFER_SIZE);check(socket->out_buffer!=nullptr);
            socket->out_buffer->append(payload.data(),length);check(socket->out_buffer->count()==1);
            check(socket->handle_send()==SW_OK);check(socket->out_buffer->empty()&&socket->out_buffer->count()==0);
            check(pair.receive(length)==payload.substr(0,length));check(!socket->send_wait&&!socket->close_wait);check(socket->out_buffer->length()==0);++cases;
        }
        OrdinaryOutputFile file(payload);
        for(off_t offset:{off_t(0),off_t(2)})for(size_t length:{size_t(0),size_t(1),size_t(7)}){
            OrdinaryOutputPair pair;auto *socket=pair.socket;check(socket&&socket->fd>=0);
            check(socket->sendfile_async(file.path,offset,length)==SW_OK);check(socket->out_buffer&&socket->out_buffer->count()==1);
#if defined(SWOOLE_RS_BUFFER_TYPED) && SWOOLE_RS_BUFFER_TYPED
            auto *task=static_cast<network::SendfileRequest*>(socket->out_buffer->typed_front().token);
#else
            auto *task=static_cast<network::SendfileRequest*>(socket->out_buffer->front()->value.ptr);
#endif
            size_t expected=length?length:payload.size()-offset;
            check(task->begin==offset&&task->end==static_cast<off_t>(offset+expected));check(task->corked==0);
            unsigned attempts=0;while(!socket->out_buffer->empty()){
                if(++attempts>16||socket->handle_sendfile()!=SW_OK)throw std::runtime_error("ordinary positive queued sendfile failed");
            }
            check(socket->out_buffer->count()==0);check(pair.receive(expected)==payload.substr(offset,expected));
            check(!socket->send_wait&&!socket->close_wait);check(socket->out_buffer->length()==0);++cases;
        }
        add_assoc_long(return_value,"cases",cases);add_assoc_zval(return_value,"outcomes",&rows);
    }catch(const std::exception &error){zval_ptr_dtor(&rows);zend_throw_exception(zend_ce_exception,error.what(),0);}
}

PHP_FUNCTION(swr_reactor_blocking_supported) {
    ZEND_PARSE_PARAMETERS_NONE();array_init(return_value);
    image(return_value,"method_image","_ZN6swoole7network6Socket9send_syncEPKvmi");
    image(return_value,"vector_image","_ZN6swoole7network6Socket11writev_syncEPK5iovecm");
    image(return_value,"event_image","_ZNK6swoole7network6Socket15what_event_wantEi");
#if defined(__APPLE__)
    image(return_value,"file_image","_ZN6swoole7network6Socket13sendfile_syncEPKcxm");
#else
    image(return_value,"file_image","_ZN6swoole7network6Socket13sendfile_syncEPKclm");
#endif
    zval rows;array_init(&rows);auto check=[&](bool value){add_next_index_bool(&rows,value);if(!value)throw std::runtime_error("ordinary blocking assertion failed");};unsigned cases=0;
    try {
        std::string payload;for(unsigned i=0;i<47;++i)payload+=static_cast<char>(i);
        auto prepared=[&](network::Socket *socket){check(socket->nonblock);check((::fcntl(socket->fd,F_GETFL)&O_NONBLOCK)!=0);check(!socket->send_wait&&!socket->close_wait);check(socket->out_buffer==nullptr);};
        for(size_t length:{size_t(1),size_t(7),size_t(31)}){
            OrdinaryOutputPair pair;auto *socket=pair.socket;check(socket&&socket->fd>=0);
            check(socket->send_sync(payload.data(),length,0)==static_cast<ssize_t>(length));check(pair.receive(length)==payload.substr(0,length));prepared(socket);++cases;
        }
        iovec vectors[]={{&payload[0],2},{&payload[2],3},{&payload[5],7}};
        {OrdinaryOutputPair pair;auto *socket=pair.socket;check(socket&&socket->fd>=0);check(socket->writev_sync(vectors,3)==12);check(pair.receive(12)==payload.substr(0,12));prepared(socket);++cases;}
        OrdinaryOutputFile file(payload);
        for(off_t offset:{off_t(0),off_t(2)}){OrdinaryOutputPair pair;auto *socket=pair.socket;check(socket&&socket->fd>=0);check(socket->sendfile_sync(file.path,offset,0)==SW_OK);check(pair.receive(payload.size()-offset)==payload.substr(offset));prepared(socket);++cases;}
        {network::IOVector v(vectors,3);check(v.length()==12&&v.count==3);check(vectors[0].iov_len==2&&vectors[1].iov_len==3&&vectors[2].iov_len==7);
            v.update_iterator(1);check(v.get_remain_count()==3&&v.get_index()==0&&v.get_offset_bytes()==1);check(v.get_iterator()->iov_base==&payload[1]&&v.get_iterator()->iov_len==1);
            v.update_iterator(1);check(v.get_remain_count()==2&&v.get_index()==1&&v.get_offset_bytes()==1);check(v.get_iterator()->iov_base==&payload[2]&&v.get_iterator()->iov_len==3);
            v.update_iterator(3);check(v.get_remain_count()==1&&v.get_index()==2&&v.get_offset_bytes()==1);check(v.get_iterator()->iov_base==&payload[5]&&v.get_iterator()->iov_len==7);
            auto *previous=v.get_iterator();v.update_iterator(7);check(v.get_remain_count()==0&&v.get_index()==3&&v.get_offset_bytes()==1);check(v.get_iterator()==previous);
            bool unchanged=true;for(int i=0;i<3;++i)unchanged=unchanged&&v.iov[i].iov_base==vectors[i].iov_base&&v.iov[i].iov_len==vectors[i].iov_len;check(unchanged);check(vectors[0].iov_len==2&&vectors[1].iov_len==3&&vectors[2].iov_len==7);++cases;}
        add_assoc_long(return_value,"cases",cases);add_assoc_zval(return_value,"outcomes",&rows);
    }catch(const std::exception &error){zval_ptr_dtor(&rows);zend_throw_exception(zend_ce_exception,error.what(),0);}
}

static const zend_function_entry functions[]={PHP_FE(swr_reactor_supported,arginfo_swr_reactor_supported) PHP_FE(swr_reactor_callbacks_supported,arginfo_swr_reactor_supported) PHP_FE(swr_reactor_builtins_supported,arginfo_swr_reactor_supported) PHP_FE(swr_reactor_output_supported,arginfo_swr_reactor_supported) PHP_FE(swr_reactor_blocking_supported,arginfo_swr_reactor_supported) PHP_FE_END};
extern "C" {
zend_module_entry swr_reactor_supported_module_entry={STANDARD_MODULE_HEADER,"swr_reactor_supported",functions,
    nullptr,nullptr,nullptr,nullptr,nullptr,"1",STANDARD_MODULE_PROPERTIES};
ZEND_GET_MODULE(swr_reactor_supported)
}
