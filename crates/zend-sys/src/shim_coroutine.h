/* Coroutine VM integration (crates/phpcoroutines): what a coroutine swaps in
 * and out of the executor, output and basic globals, whose fields depend on the
 * PHP version, and the VM internals Swoole pokes (opcodes, oplines, hooks). */
#ifndef SWOOLE_RS_ZEND_SHIM_COROUTINE_H
#define SWOOLE_RS_ZEND_SHIM_COROUTINE_H

#include "shim.h"
#include "callable.h"
#include "zend_fibers.h"
#include "zend_observer.h"
#include "zend_builtin_functions.h"

/* Swoole's PHPContext VM fields: one coroutine's executor state while another
 * runs. */
typedef struct swr_zend_coroutine_vm {
    JMP_BUF *bailout;
    zval *vm_stack_top;
    zval *vm_stack_end;
    zend_vm_stack vm_stack;
    size_t vm_stack_page_size;
    zend_execute_data *execute_data;
    uint32_t jit_trace_num;
    zend_error_handling_t error_handling;
    zend_class_entry *exception_class;
    zend_object *exception;
    const zend_op *opline_before_exception;
#ifdef ZEND_CHECK_STACK_LIMIT
    void *stack_base;
    void *stack_limit;
#endif
    zend_output_globals *output_ptr;
    /* var serialize/unserialize: a switch may happen in __sleep/__wakeup */
    unsigned serialize_lock;
    struct {
        struct php_serialize_data *data;
        unsigned level;
    } serialize;
    struct {
        struct php_unserialize_data *data;
        unsigned level;
    } unserialize;
    /* error control `@` */
    bool in_silence;
    int ori_error_reporting;
    int tmp_error_reporting;
} swr_zend_coroutine_vm;

/* save_context: the executor, output and serializer globals into `vm`. */
void swr_zend_coroutine_save(swr_zend_coroutine_vm *vm);
/* restore_context. */
void swr_zend_coroutine_restore(swr_zend_coroutine_vm *vm);
/* save_context(from) then restore_context(to): one coroutine switch. */
void swr_zend_coroutine_switch(swr_zend_coroutine_vm *from, swr_zend_coroutine_vm *to);

/* create_context's executor setup on a new coroutine: a fresh VM stack whose
 * first frame is an empty internal function, cleared exception state, the C
 * stack bounds of `stack` (null: none), then save_vm_stack into `vm`. */
void swr_zend_coroutine_start(swr_zend_coroutine_vm *vm, void *stack, size_t stack_size);
/* destroy_context's output reset: end and restart output buffering without
 * sending headers, when output is active. */
void swr_zend_coroutine_output_reset(void);
/* zend_vm_stack_destroy. */
void swr_zend_coroutine_vm_destroy(void);
/* init_main_context: EG(main_fiber_context), then save_context into `vm`. */
zend_fiber_context *swr_zend_coroutine_main(swr_zend_coroutine_vm *vm);
/* A coroutine's body (main_func) inside zend_first_try: call the function
 * `enter` returns and report an uncaught exception on this coroutine's stack
 * (zend_exception_error, E_ERROR), then run `defers`, its defer tasks, which
 * bail out on an exception; a bailout's exception is reported too. 1 if an
 * exception or bailout ended it. */
int swr_zend_coroutine_run(swr_zend_callable *(*enter)(void *context), void (*defers)(void *context),
                           void *context);
/* The VM stack bytes in use from `stack` (the running one if `running`) down
 * its chain (get_stack_usage). */
size_t swr_zend_coroutine_stack_usage(zend_vm_stack stack, bool running);

/* EG(error_reporting). */
int swr_zend_error_reporting(void);

typedef void (*swr_zend_interrupt_handler)(zend_execute_data *execute_data);
/* zend_interrupt_function = handler; returns the previous one. */
swr_zend_interrupt_handler swr_zend_coroutine_set_interrupt(swr_zend_interrupt_handler handler);
/* &EG(vm_interrupt), for the interrupt thread. */
zend_atomic_bool *swr_zend_vm_interrupt(void);
/* zend_atomic_bool_store(flag, 1), from any thread. */
void swr_zend_vm_interrupt_raise(zend_atomic_bool *flag);

typedef zend_class_entry *(*swr_zend_autoload_handler)(zend_string *name, zend_string *lc_name);
/* zend_autoload = handler; returns the previous one. */
swr_zend_autoload_handler swr_zend_coroutine_set_autoload(swr_zend_autoload_handler handler);
/* zend_hash_del(EG(in_autoload), lc_name). */
void swr_zend_autoload_forget(zend_string *lc_name);
/* *result = handler(name, lc_name) inside zend_try: 1 if it bailed out. */
int swr_zend_autoload_try(swr_zend_autoload_handler handler, zend_string *name, zend_string *lc_name,
                          zend_class_entry **result);

/* zend_set_user_opcode_handler for ZEND_EXIT (before PHP 8.4, else ignored),
 * ZEND_BEGIN_SILENCE and ZEND_END_SILENCE. */
void swr_zend_coroutine_opcodes(user_opcode_handler_t exit_handler, user_opcode_handler_t begin_silence,
                                user_opcode_handler_t end_silence);
/* The exit opcode's operand, dereferenced and duplicated into `status`
 * (ZVAL_DUP), or null when it has none. */
void swr_zend_exit_status(zend_execute_data *execute_data, zval *status);

/* zend_parse_arg_str_or_long_slow inside zend_try (a __toString may run):
 * 1 if it bailed out; *parsed says whether it parsed. */
int swr_zend_try_parse_str_or_long(zval *arg, zend_string **string, zend_long *number_out, uint32_t number,
                                   bool *parsed);

/* EG(current_execute_data) = frame; returns the previous one. */
zend_execute_data *swr_zend_coroutine_set_frame(zend_execute_data *frame);
/* Whether the function that called the running internal function is named
 * "__destruct" (go() during shutdown functions); false without a current
 * frame, a caller's frame or a function in it. */
bool swr_zend_coroutine_called_by_destructor(void);
/* Whether the constant SWOOLE_LIBRARY is defined and true (zend_is_true). */
bool swr_zend_coroutine_library_loaded(void);
/* sw_zend_fci_cache_discard: release the bound object and closure that
 * swr_zend_callable_hold held. */
void swr_zend_coroutine_fcc_discard(zend_fcall_info_cache *fcc);

/* sw_php_print_backtrace_impl(skip_last, options, limit) inside zend_try
 * (output handlers may run): 1 if it bailed out. */
int swr_zend_coroutine_print_trace(int skip_last, int options, int limit);

/* Set an exception's file and line from the current frame, as
 * zend::object_set(ZSTR_KNOWN(ZEND_STR_FILE/LINE)) does. */
void swr_zend_coroutine_locate(zend_object *exception);
/* Make the suspended frame in `vm` throw on resume, as
 * zend_throw_exception_internal() does for a user frame. */
void swr_zend_coroutine_redirect(swr_zend_coroutine_vm *vm);
#endif
