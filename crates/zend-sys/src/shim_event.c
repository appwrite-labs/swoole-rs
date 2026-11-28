/* Stays C (docs/zend.md, What stays C): globals (EG(flags), CG(class_table)),
 * version (EG_FLAGS_*), bailout (__get) and private (ext/sockets' php_socket,
 * present only when PHP has it). */
#include "shim.h"
#include "shim_event.h"
/* php_sockets.h is empty unless HAVE_SOCKETS is set, which a distro PHP built with
 * ext/sockets as a shared module does not define in php_config.h. */
#if __has_include("ext/sockets/php_sockets.h")
#ifndef HAVE_SOCKETS
#define HAVE_SOCKETS 1
#endif
#include "ext/sockets/php_sockets.h"
#endif

uint8_t swr_zend_event_pause_shutdown(void) {
#if defined(EG_FLAGS_IN_SHUTDOWN) && !defined(EG_FLAGS_OBJECT_STORE_NO_REUSE)
    uint8_t previous = (EG(flags) & EG_FLAGS_IN_SHUTDOWN) != 0;
    EG(flags) &= ~EG_FLAGS_IN_SHUTDOWN;
    return previous;
#else
    return 0;
#endif
}

void swr_zend_event_restore_shutdown(uint8_t previous) {
#if defined(EG_FLAGS_IN_SHUTDOWN) && !defined(EG_FLAGS_OBJECT_STORE_NO_REUSE)
    if (previous) EG(flags) |= EG_FLAGS_IN_SHUTDOWN;
#else
    (void) previous;
#endif
}

int swr_zend_event_property(zval *object, zend_string *name, zval **result) {
    volatile int bailed = 0;
    SWR_ZEND_TRY {
        *result = zend_read_property_ex(Z_OBJCE_P(object), Z_OBJ_P(object), name, 0, NULL);
    } SWR_ZEND_CATCH {
        bailed = 1;
    } SWR_ZEND_END_TRY();
    return bailed;
}

int swr_zend_event_socket_fd(zval *object, uint8_t *closed) {
#ifdef HAVE_SOCKETS
    /* socket_ce, found by name: linking it would make ext/sockets a load-time dependency. */
    zend_class_entry *socket = zend_hash_str_find_ptr(CG(class_table), ZEND_STRL("socket"));
    if (socket && instanceof_function(Z_OBJCE_P(object), socket)) {
        php_socket *php_sock = Z_SOCKET_P(object);
        *closed = IS_INVALID_SOCKET(php_sock);
        return php_sock->bsd_socket;
    }
#else
    (void) object;
    (void) closed;
#endif
    return -2;
}
