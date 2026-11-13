#ifndef SWOOLE_RS_PHPHELPERS_H
#define SWOOLE_RS_PHPHELPERS_H
#include <stddef.h>
#ifdef __cplusplus
extern "C" {
#endif
/* The class alias the Zend class-entry macros register through Rust. */
struct _zend_class_entry;
int swr_v1_phphelpers_class_alias(const char *name, size_t length, struct _zend_class_entry *entry);
#ifdef __cplusplus
}
#endif
#endif
