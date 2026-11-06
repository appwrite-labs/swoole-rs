#ifndef SWOOLE_RS_ZEND_SHIM_FACADE_H
#define SWOOLE_RS_ZEND_SHIM_FACADE_H
#include "php.h"
#include "zend_exceptions.h"
/* User code invoked by object bool casts and callable resolution stays in C. */
int swr_zend_try_facade_truth(zval *, bool *);
int swr_zend_try_facade_callable(zval *, zend_fcall_info_cache *, bool *);
int swr_zend_try_facade_name(zval *, zend_string **);
uint32_t swr_zval_facade_refcount(const zval *);

int swr_zend_try_facade_throw(zend_class_entry *, const char *, zend_long);

#endif
