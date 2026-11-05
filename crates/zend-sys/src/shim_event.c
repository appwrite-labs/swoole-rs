#include "shim_event.h"

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
    zend_try {
        *result = zend_read_property_ex(Z_OBJCE_P(object), Z_OBJ_P(object), name, 0, NULL);
    } zend_catch {
        bailed = 1;
    } zend_end_try();
    return bailed;
}
