/* Stays C (docs/zend.md, What stays C): globals (the executing file's name is
 * the executor's current frame's). */
#include "shim.h"
#include "shim_process.h"

key_t swr_zend_process_ftok(void) {
    return ftok(zend_get_executed_filename(), 1);
}
