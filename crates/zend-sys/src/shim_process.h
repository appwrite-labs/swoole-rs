/* Swoole\Process's reads of the executor (crates/phpprocess). */
#ifndef SWOOLE_RS_ZEND_SHIM_PROCESS_H
#define SWOOLE_RS_ZEND_SHIM_PROCESS_H

#include "shim.h"
#include <sys/ipc.h>

/* ftok(zend_get_executed_filename(), 1): the System V key of the file whose
 * code is running, -1 when it has none ("[no active file]") or cannot be read. */
key_t swr_zend_process_ftok(void);
#endif
