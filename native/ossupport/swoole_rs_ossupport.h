/* SPDX-License-Identifier: Apache-2.0 */
#ifndef SWOOLE_RS_OSSUPPORT_H
#define SWOOLE_RS_OSSUPPORT_H
#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>
#include <sys/types.h>
#ifdef __cplusplus
extern "C" {
#endif
/* Rust-owned File state. */
typedef struct swr_v1_os_file_record swr_v1_os_file_record;
swr_v1_os_file_record *swr_v1_os_file_record_open(const char *path, int flags, int mode);
void swr_v1_os_file_record_free(swr_v1_os_file_record *owner);
int swr_v1_os_file_record_fd(swr_v1_os_file_record *owner, bool release);
/* file_get_contents and File::read_content; `content` is a
 * swoole::String (default-constructed, or File's fresh one). */
bool swr_v1_os_file_get_contents(const char *path, void *content);
void swr_v1_os_file_read_content(const swr_v1_os_file_record *owner, void *content);
/* swoole::MsgQueue (QueueRecord in Rust), a SysV message queue with upstream's
 * warnings; Rust owns and frees it. A message is
 * a QueueNode: its long type, then the payload. */
typedef struct swr_v1_os_msg_queue swr_v1_os_msg_queue;
#ifdef __cplusplus
}
static_assert(sizeof(uintptr_t) == sizeof(void *), "opaque token width");
#endif
#endif
