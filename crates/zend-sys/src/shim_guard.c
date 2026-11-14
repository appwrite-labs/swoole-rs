/* The native scope registry. Stays C (docs/zend.md, What stays C): bailout (the
 * guard macros around zend_try call it) and globals (ZEND_TLS, EG(bailout)). */
#include "shim.h"

/* Registration is published once in MINIT, before starting any PHP threads. */
static void *(*current_context)(void);
static void (*resume_exit)(void);
ZEND_TLS volatile swr_zend_guard *guards;
ZEND_TLS int thread_exit;

void swr_zend_guard_register(void *(*context)(void), void (*resume)(void)) {
    current_context = context;
    resume_exit = resume;
}

void swr_zend_guard_enter(volatile swr_zend_guard *guard, void *target) {
    guard->context = current_context ? current_context() : NULL;
    guard->target = target;
    guard->previous = guards;
    guards = guard;
}

void swr_zend_guard_leave(volatile swr_zend_guard *guard) {
    volatile swr_zend_guard *entry = guards;
    volatile swr_zend_guard *previous = NULL;
    while (entry && entry != guard) {
        previous = entry;
        entry = entry->previous;
    }
    if (entry) {
        if (previous) previous->previous = guard->previous;
        else guards = guard->previous;
    }
    guard->target = NULL;
}

void swr_zend_guard_scope_cleanup(volatile swr_zend_guard *guard) {
    void *target = guard->target;
    if (!target) return;
    swr_zend_guard_leave(guard);
    if ((void *) EG(bailout) == target) EG(bailout) = guard->original_bailout;
}

void *swr_zend_guard_target(void) {
    void *context = current_context ? current_context() : NULL;
    for (volatile swr_zend_guard *guard = guards; guard; guard = guard->previous) {
        if (guard->context == context) return guard->target;
    }
    return NULL;
}

int swr_zend_guard_pending(void) { return thread_exit; }
void swr_zend_guard_mark(void) { thread_exit = 2; }
void swr_zend_guard_clear(void) { thread_exit = 0; }

void swr_zend_guard_resume(void) {
    if (thread_exit && resume_exit) resume_exit();
    zend_bailout();
}
