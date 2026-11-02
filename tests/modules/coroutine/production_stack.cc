// Complete optional marker constructor/query callers, never a context switch.
extern "C" {
#include <php.h>
#include <Zend/zend_exceptions.h>
}
#include <dlfcn.h>
#include <cstring>
#include <stdexcept>
#include "actual-context.inc"
#ifdef SW_USE_THREAD_CONTEXT
#error This witness does not exercise thread contexts
#endif

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_swr_context_markers,0,0,IS_ARRAY,0)
ZEND_END_ARG_INFO()
PHP_FUNCTION(swr_context_markers) {
    ZEND_PARSE_PARAMETERS_NONE();
    unsigned checks=0,contexts=0,callbacks=0;
    auto check=[&](bool valid){++checks;if(!valid)throw std::runtime_error("Context marker observation differs");};
    try {
        constexpr size_t baseline=64*1024;
        constexpr char magic[]="swoole_coroutine#5652a7fb2b38be";
        for(size_t size:{baseline,baseline+baseline/2,2*baseline,4*baseline,16*baseline,64*baseline}) {
            swoole::coroutine::Context context(size,[&](void*){++callbacks;},nullptr);
            ++contexts;
            check(!context.is_end());check(context.get_stack()!=nullptr);
            check(context.get_stack_size()==size);check(context.get_stack_usage()==ssize_t(baseline));
            // Only initialized sampled marker bytes, away from a protected first page.
            for(size_t offset=baseline;offset<size;offset*=2) {
                char *marker=context.get_stack()+size-offset+sizeof(magic)-1;
                check(std::memcmp(marker,magic,sizeof(magic)-1)==0);
                marker[0]^=1;
                check(context.get_stack_usage()==ssize_t(2*offset));
                marker[0]^=1;
                check(context.get_stack_usage()==ssize_t(baseline));
            }
            check(callbacks==0);
        }
        check(callbacks==0);
        array_init(return_value);add_assoc_long(return_value,"contexts",contexts);
        add_assoc_long(return_value,"checks",checks);add_assoc_long(return_value,"callbacks",callbacks);
        add_assoc_long(return_value,"class_size",sizeof(swoole::coroutine::Context));
        add_assoc_long(return_value,"class_alignment",alignof(swoole::coroutine::Context));
#ifdef SWOOLE_RS_COROUTINE
        Dl_info info{};void *address=dlsym(RTLD_DEFAULT,"swr_v1_context_stack_alloc");
        if(!address||!dladdr(address,&info))throw std::runtime_error("Rust marker symbol missing");
        add_assoc_string(return_value,"marker_image",const_cast<char*>(info.dli_fname));
#endif
    } catch(const std::exception &error) {zend_throw_exception(zend_ce_exception,error.what(),0);}
}
static const zend_function_entry functions[]={PHP_FE(swr_context_markers,arginfo_swr_context_markers) PHP_FE_END};
extern "C" {
ZEND_DLEXPORT zend_module_entry context_marker_module_entry={STANDARD_MODULE_HEADER,"swr_context_markers",functions,nullptr,nullptr,nullptr,nullptr,nullptr,"1",STANDARD_MODULE_PROPERTIES};
ZEND_GET_MODULE(context_marker)
}
