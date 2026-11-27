/* The coroutine VM switch. Stays C (docs/zend.md, What stays C): globals (the EG,
 * OG, BG and SG state each coroutine saves and restores), bailout (zend_first_try
 * around a coroutine, autoloaders, backtraces), version (ZEND_EXIT before PHP
 * 8.4, ZEND_CHECK_STACK_LIMIT), allocator, budget and variadic (printf). */
#include "shim.h"
#include "shim_coroutine.h"
#include "shim_private.h"

#include "zend_builtin_functions.h"
#include "zend_vm.h"
#include "ext/standard/basic_functions.h"
#include "main/php_output.h"
#include "SAPI.h"
#include <stdio.h>

ZEND_TLS int default_error_reporting = E_ALL;

#define SWR_COROUTINE_PAGE_SIZE 8192
#define SWR_INVALID_PTR ((void *) -1)

/* globals: snapshot EG, including stack-limit fields present in checked builds. */
static void save_vm_stack(swr_zend_coroutine_vm *vm) {
    vm->bailout = EG(bailout);
    vm->vm_stack_top = EG(vm_stack_top);
    vm->vm_stack_end = EG(vm_stack_end);
    vm->vm_stack = EG(vm_stack);
    vm->vm_stack_page_size = EG(vm_stack_page_size);
    vm->execute_data = EG(current_execute_data);
    vm->jit_trace_num = EG(jit_trace_num);
    vm->error_handling = EG(error_handling);
    vm->exception_class = EG(exception_class);
    vm->exception = EG(exception);
    vm->opline_before_exception = EG(opline_before_exception);
    vm->error_reporting = EG(error_reporting);
#ifdef ZEND_CHECK_STACK_LIMIT
    vm->stack_base = EG(stack_base);
    vm->stack_limit = EG(stack_limit);
#endif
}

/* globals: restore the executor slots for this PHP thread. */
static void restore_vm_stack(swr_zend_coroutine_vm *vm) {
    EG(bailout) = vm->bailout;
    EG(vm_stack_top) = vm->vm_stack_top;
    EG(vm_stack_end) = vm->vm_stack_end;
    EG(vm_stack) = vm->vm_stack;
    EG(vm_stack_page_size) = vm->vm_stack_page_size;
    EG(current_execute_data) = vm->execute_data;
    EG(jit_trace_num) = vm->jit_trace_num;
    EG(error_handling) = vm->error_handling;
    EG(exception_class) = vm->exception_class;
    EG(exception) = vm->exception;
    EG(opline_before_exception) = vm->opline_before_exception;
    EG(error_reporting) = vm->error_reporting;
#ifdef ZEND_CHECK_STACK_LIMIT
    EG(stack_base) = vm->stack_base;
    EG(stack_limit) = vm->stack_limit;
#endif
}

#define SWOG ((zend_output_globals *) &OG(handlers))

/* globals: suspend OG output and BG serialization state alongside EG. */
void swr_zend_coroutine_save(swr_zend_coroutine_vm *vm) {
    save_vm_stack(vm);
    if (OG(handlers).elements) {
        vm->output_ptr = (zend_output_globals *) emalloc(sizeof(zend_output_globals));
        memcpy(vm->output_ptr, SWOG, sizeof(zend_output_globals));
        php_output_activate();
    } else {
        vm->output_ptr = NULL;
    }
    if (BG(serialize_lock)) {
        vm->serialize_lock = BG(serialize_lock);
    }
    if (BG(serialize).data) {
        memcpy(&vm->serialize, &BG(serialize), sizeof(BG(serialize)));
    }
    if (BG(unserialize).data) {
        memcpy(&vm->unserialize, &BG(unserialize), sizeof(BG(unserialize)));
    }
}

/* globals: reinstall OG and BG state and release the saved output allocation. */
void swr_zend_coroutine_restore(swr_zend_coroutine_vm *vm) {
    restore_vm_stack(vm);
    if (vm->output_ptr) {
        memcpy(SWOG, vm->output_ptr, sizeof(zend_output_globals));
        efree(vm->output_ptr);
        vm->output_ptr = NULL;
    }
    if (vm->serialize_lock) {
        BG(serialize_lock) = vm->serialize_lock;
        vm->serialize_lock = 0;
    }
    if (vm->serialize.data) {
        memcpy(&BG(serialize), &vm->serialize, sizeof(BG(serialize)));
        memset(&vm->serialize, 0, sizeof(vm->serialize));
    }
    if (vm->unserialize.data) {
        memcpy(&BG(unserialize), &vm->unserialize, sizeof(BG(unserialize)));
        memset(&vm->unserialize, 0, sizeof(vm->unserialize));
    }
}

/* The function of each coroutine's first frame (swoole_coroutine_internal_function). */
static zend_function frame_function = {ZEND_INTERNAL_FUNCTION};

/* globals: install a new EG stack and first frame with this build's stack bounds. */
void swr_zend_coroutine_start(swr_zend_coroutine_vm *from, swr_zend_coroutine_vm *vm, void *stack,
                              size_t stack_size) {
    swr_zend_coroutine_save(from);
    EG(error_reporting) = default_error_reporting;
    EG(vm_stack) = zend_vm_stack_new_page(SWR_COROUTINE_PAGE_SIZE, NULL);
    EG(vm_stack_top) = EG(vm_stack)->top + ZEND_CALL_FRAME_SLOT;
    EG(vm_stack_end) = EG(vm_stack)->end;
    EG(vm_stack_page_size) = SWR_COROUTINE_PAGE_SIZE;

    zend_execute_data *call = (zend_execute_data *) EG(vm_stack_top);
    EG(current_execute_data) = call;
    memset(EG(current_execute_data), 0, sizeof(zend_execute_data));

    EG(error_handling) = EH_NORMAL;
    EG(exception_class) = NULL;
    EG(exception) = NULL;
    EG(opline_before_exception) = NULL;
    EG(jit_trace_num) = 0;

    call->func = &frame_function;
    EG(vm_stack_top) += ZEND_CALL_FRAME_SLOT;

#ifdef ZEND_CHECK_STACK_LIMIT
    if (stack) {
        zend_ulong reserve = EG(reserved_stack_size);
#ifdef __APPLE__
        /* Apple Clang's ___chkstk_darwin probes twice the frame size away from
         * the stack pointer on an alternative stack. */
        reserve = reserve * 2;
#endif
        EG(stack_base) = (void *) ((uintptr_t) stack + stack_size);
        EG(stack_limit) = (int8_t *) stack + reserve;
    } else {
        EG(stack_base) = NULL;
        EG(stack_limit) = NULL;
    }
#else
    (void) stack;
    (void) stack_size;
#endif
    save_vm_stack(vm);
}

/* bailout: output handlers run PHP under a native jump target. */
static int reset_output_handlers(void) {
    volatile int bailed = 0;
    zend_bool no_headers = SG(request_info).no_headers;
    /* Do not send headers by SAPI */
    SG(request_info).no_headers = true;
    SWR_ZEND_TRY {
        if (OG(active)) {
            php_output_end_all();
        }
        php_output_deactivate();
        php_output_activate();
    }
    SWR_ZEND_CATCH { bailed = 1; }
    SWR_ZEND_END_TRY();
    SG(request_info).no_headers = no_headers;
    return bailed;
}

/* globals: inspect OG before installing the output reset trap. */
int swr_zend_coroutine_output_reset(void) {
    return OG(handlers).elements ? reset_output_handlers() : 0;
}

static void throw_exit(void *exception) { zend_throw_exception_internal(exception); }

/* globals, bailout: transfer an exit to the restored frame inside a native trap. */
int swr_zend_coroutine_finish(swr_zend_coroutine_vm *finished, swr_zend_coroutine_vm *origin) {
    zend_object *exit = swr_zend_unwind_exit_pending() ? EG(exception) : NULL;
    zend_vm_stack_destroy();
    finished->vm_stack = NULL;
    finished->execute_data = NULL;
    swr_zend_coroutine_restore(origin);
    return exit ? swr_zend_try(throw_exit, exit) : SWR_ZEND_RETURN;
}

/* globals: return this executor's main fiber after saving its state. */
zend_fiber_context *swr_zend_coroutine_main(swr_zend_coroutine_vm *vm) {
    EG(opline_before_exception) = NULL;
    swr_zend_coroutine_save(vm);
    return EG(main_fiber_context);
}

/* globals: report the current EG exception from inside the native trap. */
static int coroutine_catch(void) {
    if (UNEXPECTED(EG(exception)) && !swr_zend_unwind_exit_pending()) {
        zend_exception_error(EG(exception), E_ERROR);
        return 1;
    }
    return 0;
}

/* bailout: the entry, callable and deferred handlers share zend_first_try. */
int swr_zend_coroutine_run(swr_zend_callable *(*enter)(void *context), void (*defers)(void *context),
                           void *context) {
    volatile int caught = 0;
    zend_first_try {
        swr_zend_callable *call = enter(context);
        if (call) {
            zend_call_function(&call->fci, &call->fcc);
        }
        caught = coroutine_catch();
        defers(context);
    }
    zend_catch {
        coroutine_catch();
        caught = 1;
    }
    zend_end_try();
    return caught;
}

/* globals: the running coroutine's stack starts at EG(vm_stack). */
size_t swr_zend_coroutine_stack_usage(const swr_zend_coroutine_vm *vm) {
    zend_vm_stack stack = vm ? vm->vm_stack : EG(vm_stack);
    size_t usage = 0;
    while (stack) {
        usage += (stack->end - stack->top) * sizeof(zval);
        stack = stack->prev;
    }
    return usage;
}

/* globals: a new environment uses the configured level even when activated under @. */
void swr_zend_coroutine_snapshot_reporting(void) {
    if (!EG(error_reporting_ini_entry)) {
        EG(error_reporting_ini_entry) = zend_hash_find_ptr(EG(ini_directives), ZSTR_KNOWN(ZEND_STR_ERROR_REPORTING));
    }
    const zend_ini_entry *entry = EG(error_reporting_ini_entry);
    default_error_reporting = entry && entry->value ? atoi(ZSTR_VAL(entry->value)) : E_ALL;
}

/* budget: replacing Zend's writable function pointer in Rust requires unsafe. */
swr_zend_interrupt_handler swr_zend_coroutine_set_interrupt(swr_zend_interrupt_handler handler) {
    swr_zend_interrupt_handler previous = zend_interrupt_function;
    zend_interrupt_function = handler;
    return previous;
}

/* globals: take the current thread's EG interrupt flag. */
zend_atomic_bool *swr_zend_vm_interrupt(void) {
    return &EG(vm_interrupt);
}

/* budget: exchanging the writable zend_autoload hook requires unsafe in Rust. */
swr_zend_autoload_handler swr_zend_coroutine_set_autoload(swr_zend_autoload_handler handler) {
    swr_zend_autoload_handler previous = zend_autoload;
    zend_autoload = handler;
    return previous;
}

/* globals: remove this name from the current executor's autoload table. */
void swr_zend_autoload_forget(zend_string *lc_name) {
    if (EG(in_autoload)) {
        zend_hash_del(EG(in_autoload), lc_name);
    }
}

/* bailout: a user autoloader can leave through zend_bailout. */
int swr_zend_autoload_try(swr_zend_autoload_handler handler, zend_string *name, zend_string *lc_name,
                          zend_class_entry **result) {
    volatile int bailed = 0;
    SWR_ZEND_TRY {
        *result = handler(name, lc_name);
    }
    SWR_ZEND_CATCH {
        bailed = 1;
    }
    SWR_ZEND_END_TRY();
    return bailed;
}

/* version: ZEND_EXIT has an opcode handler only before PHP 8.4. */
void swr_zend_coroutine_opcodes(user_opcode_handler_t exit_handler) {
#if PHP_VERSION_ID < 80400
    _Static_assert(ZEND_EXIT == 79, "crates/phpcoroutines' Exit::OPCODE");
    zend_set_user_opcode_handler(ZEND_EXIT, exit_handler);
#else
    (void) exit_handler;
#endif
}

/* budget: the opcode selects a raw literal or frame slot before ZVAL_DUP. */
void swr_zend_exit_status(zend_execute_data *execute_data, zval *status) {
    const zend_op *opline = EX(opline);
    if (opline->op1_type != IS_UNUSED) {
        zval *exit_status;
        if (opline->op1_type == IS_CONST) {
            exit_status = RT_CONSTANT(opline, opline->op1);
        } else {
            exit_status = EX_VAR(opline->op1.var);
        }
        if (Z_ISREF_P(exit_status)) {
            exit_status = Z_REFVAL_P(exit_status);
        }
        ZVAL_DUP(status, exit_status);
    } else {
        ZVAL_NULL(status);
    }
}

/* bailout: slow argument conversion can invoke PHP. */
int swr_zend_try_parse_str_or_long(zval *arg, zend_string **string, zend_long *number_out, uint32_t number,
                                   bool *parsed) {
    volatile int bailed = 0;
    *parsed = false;
    SWR_ZEND_TRY {
        *parsed = zend_parse_arg_str_or_long_slow(arg, string, number_out, number);
    }
    SWR_ZEND_CATCH {
        bailed = 1;
    }
    SWR_ZEND_END_TRY();
    return bailed;
}

/* globals: swap EG(current_execute_data) on the running thread. */
zend_execute_data *swr_zend_coroutine_set_frame(zend_execute_data *frame) {
    zend_execute_data *previous = EG(current_execute_data);
    EG(current_execute_data) = frame;
    return previous;
}

/* globals: expose a suspended frame through EG(current_execute_data). */
zend_execute_data *swr_zend_coroutine_show(swr_zend_coroutine_vm *vm) {
    zend_execute_data *previous = EG(current_execute_data);
    EG(current_execute_data) = vm->execute_data;
    return previous;
}

/* globals: traverse the caller of EG(current_execute_data). */
bool swr_zend_coroutine_called_by_destructor(void) {
    const zend_execute_data *frame = EG(current_execute_data);
    const zend_execute_data *caller = frame ? frame->prev_execute_data : NULL;
    const zend_function *func = caller ? caller->func : NULL;
    return func && func->common.function_name &&
           zend_string_equals_literal(func->common.function_name, "__destruct");
}

/* bailout: output handlers run PHP while writing the trace. */
int swr_zend_coroutine_print_trace(zval *backtrace) {
    volatile int bailed = 0;
    SWR_ZEND_TRY {
        zend_string *str = zend_trace_to_string(Z_ARRVAL_P(backtrace), false);
        ZEND_WRITE(ZSTR_VAL(str), ZSTR_LEN(str));
        zend_string_release(str);
    }
    SWR_ZEND_CATCH {
        bailed = 1;
    }
    SWR_ZEND_END_TRY();
    return bailed;
}

/* budget: Fiber holds a shared raw context pointer across observer calls. */
void swr_zend_coroutine_fiber_status(zend_fiber_context *fiber, zend_fiber_status status) {
    fiber->status = status;
}

/* budget: the exception handle is opaque to Rust; the update needs its class pointer. */
void swr_zend_coroutine_locate(zend_object *exception) {
    zend_string *filename = zend_get_executed_filename_ex();
    if (filename) {
        zval tmp;
        ZVAL_STR(&tmp, filename);
        zend_update_property_ex(exception->ce, exception, ZSTR_KNOWN(ZEND_STR_FILE), &tmp);
        ZVAL_LONG(&tmp, zend_get_executed_lineno());
        zend_update_property_ex(exception->ce, exception, ZSTR_KNOWN(ZEND_STR_LINE), &tmp);
    }
}

/* globals: redirect the saved frame to EG(exception_op). */
void swr_zend_coroutine_redirect(swr_zend_coroutine_vm *vm) {
    zend_execute_data *frame = vm->execute_data;
    if (frame && frame->func && ZEND_USER_CODE(frame->func->type) && frame->opline->opcode != ZEND_HANDLE_EXCEPTION) {
        vm->opline_before_exception = frame->opline;
        frame->opline = EG(exception_op);
    }
}

/* variadic: print the count through libc's buffered printf. */
void swr_zend_coroutine_deadlock_report(unsigned long count) {
    printf("\n ===================================================================\n"
           "  [FATAL ERROR]: all coroutines (count: %lu) are asleep - deadlock! \n"
           " ===================================================================\n",
           count);
}
