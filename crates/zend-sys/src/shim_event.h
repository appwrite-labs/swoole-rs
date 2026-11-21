#ifndef SWOOLE_RS_ZEND_SHIM_EVENT_H
#define SWOOLE_RS_ZEND_SHIM_EVENT_H
#include "php.h"
/* Executor shutdown flags differ between SDK versions. No PHP code runs here. */
uint8_t swr_zend_event_pause_shutdown(void);
void swr_zend_event_restore_shutdown(uint8_t previous);
/* zend_read_property_ex, including any magic-method bailout, contained in C. */
int swr_zend_event_property(zval *object, zend_string *name, zval **result);
/* An ext/sockets Socket object's descriptor, with *closed set when it was
 * closed; -2 when object is no Socket or PHP has no ext/sockets. */
int swr_zend_event_socket_fd(zval *object, uint8_t *closed);
#endif
