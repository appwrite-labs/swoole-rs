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
struct stat;
/* AsyncFile owns its descriptor in Rust. The native close effect
 * ends before the owner is freed; all remaining IO uses the coroutine C ABI. */
typedef struct swr_v1_os_async_file swr_v1_os_async_file;
swr_v1_os_async_file *swr_v1_os_async_file_new(void);
swr_v1_os_async_file *swr_v1_os_async_file_clone(const swr_v1_os_async_file *);
void swr_v1_os_async_file_assign(const swr_v1_os_async_file *, const swr_v1_os_async_file *);
void swr_v1_os_async_file_free(swr_v1_os_async_file *);
int swr_v1_os_async_file_fd(const swr_v1_os_async_file *);
bool swr_v1_os_async_file_open(const swr_v1_os_async_file *, const char *, int, mode_t, int (*effect)(const char *, int, mode_t));
ssize_t swr_v1_os_async_file_read(const swr_v1_os_async_file *, void *, size_t, ssize_t (*effect)(int, void *, size_t));
ssize_t swr_v1_os_async_file_write(const swr_v1_os_async_file *, const void *, size_t, ssize_t (*effect)(int, const void *, size_t));
bool swr_v1_os_async_file_sync(const swr_v1_os_async_file *, int (*effect)(int));
bool swr_v1_os_async_file_truncate(const swr_v1_os_async_file *, off_t, int (*effect)(int, off_t));
bool swr_v1_os_async_file_stat(const swr_v1_os_async_file *, struct stat *, int (*effect)(int, struct stat *));
off_t swr_v1_os_async_file_offset(const swr_v1_os_async_file *, off_t, int, off_t (*effect)(int, off_t, int));
/* Rust-owned File state; Native path-view updates retain exceptions outside Rust. */
struct stat;
typedef struct swr_v1_os_file_record swr_v1_os_file_record;
typedef int (*swr_v1_os_file_path_update)(void *context);
swr_v1_os_file_record *swr_v1_os_file_record_new(int fd, const char *path, size_t length);
swr_v1_os_file_record *swr_v1_os_file_record_clone(const swr_v1_os_file_record *source);
void swr_v1_os_file_record_free(swr_v1_os_file_record *owner);
int swr_v1_os_file_record_fd(swr_v1_os_file_record *owner, bool release);
bool swr_v1_os_file_record_close(swr_v1_os_file_record *owner);
int swr_v1_os_file_record_open(swr_v1_os_file_record *owner, const char *path, size_t length, int flags, int mode, swr_v1_os_file_path_update update, void *context);
int swr_v1_os_file_record_assign(swr_v1_os_file_record *owner, const swr_v1_os_file_record *source, swr_v1_os_file_path_update update, void *context);
ssize_t swr_v1_os_file_record_io(const swr_v1_os_file_record *owner, uint32_t operation, void *buffer, size_t length, off_t offset);
off_t swr_v1_os_file_record_control(const swr_v1_os_file_record *owner, uint32_t operation, off_t value);
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
