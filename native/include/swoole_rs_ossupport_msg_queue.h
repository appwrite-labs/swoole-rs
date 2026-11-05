#ifndef SWOOLE_RS_OSSUPPORT_MSG_QUEUE_H
#define SWOOLE_RS_OSSUPPORT_MSG_QUEUE_H

/* Generated from crates/ossupport/src/msg_queue.rs by cbindgen; do not edit. */

#include <stdbool.h>
#include <stdint.h>
#include <stddef.h>
#include <sys/types.h>

typedef struct swr_v1_os_msg_queue {
  bool blocking;
  int id;
  key_t key;
  int flags;
  int perms;
} swr_v1_os_msg_queue;



#endif  /* SWOOLE_RS_OSSUPPORT_MSG_QUEUE_H */
