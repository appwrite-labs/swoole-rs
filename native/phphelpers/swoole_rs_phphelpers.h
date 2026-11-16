#ifndef SWOOLE_RS_PHPHELPERS_H
#define SWOOLE_RS_PHPHELPERS_H
#include <stddef.h>
#include <sys/uio.h>
#ifdef __cplusplus
extern "C" {
#endif
/* The class alias the Zend class-entry macros register through Rust: `name` lends
 * its bytes for the call. */
struct _zend_class_entry;
int swr_v1_phphelpers_class_alias(struct iovec name, struct _zend_class_entry *entry);
#ifdef __cplusplus
}
#endif
#endif
