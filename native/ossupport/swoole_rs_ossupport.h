/* SPDX-License-Identifier: Apache-2.0 */
#ifndef SWOOLE_RS_OSSUPPORT_H
#define SWOOLE_RS_OSSUPPORT_H
#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>
#include <sys/types.h>
#include "swoole_rs_ossupport_msg_queue.h"
#ifdef __cplusplus
extern "C" {
#endif
/* The process-wide async thread pool. Event tokens are native AsyncEvent
 * pointers: native runs, sends and frees them through the hooks. */
typedef struct swr_v1_os_pool swr_v1_os_pool;
typedef struct {
    void (*thread_init)(void);
    void (*thread_clean)(void);
    void (*execute)(uintptr_t event);
    void (*retire)(uintptr_t key);
    void (*spawn_failed)(void);
} swr_v1_os_pool_hooks;
const swr_v1_os_pool *swr_v1_os_pool_acquire(swr_v1_os_pool_hooks hooks, size_t cpu, size_t core,
                                             size_t workers, double wait, double idle);
void swr_v1_os_pool_release(const swr_v1_os_pool *pool);
size_t swr_v1_os_pool_task_id(const swr_v1_os_pool *pool);
void swr_v1_os_pool_dispatch(const swr_v1_os_pool *pool, uintptr_t event);
bool swr_v1_os_pool_join(const swr_v1_os_pool *pool, uintptr_t key);
size_t swr_v1_os_pool_worker_num(const swr_v1_os_pool *pool);
size_t swr_v1_os_pool_queue_size(const swr_v1_os_pool *pool);
void swr_v1_os_pool_notify_one(const swr_v1_os_pool *pool);
/* Reads up to 128 completed event tokens from the pipe and hands each to
 * complete; returns the count, or -1 with errno intact. */
ssize_t swr_v1_os_pool_complete(int fd, void (*complete)(uintptr_t event));
struct stat;
/* AsyncFile owns its descriptor in Rust. The native close effect
 * ends before the owner is freed; all remaining IO uses the coroutine C ABI. */
typedef struct swr_v1_os_async_file swr_v1_os_async_file;
swr_v1_os_async_file *swr_v1_os_async_file_new(void);
swr_v1_os_async_file *swr_v1_os_async_file_clone(const swr_v1_os_async_file *);
void swr_v1_os_async_file_assign(swr_v1_os_async_file *, const swr_v1_os_async_file *);
void swr_v1_os_async_file_free(swr_v1_os_async_file *);
int swr_v1_os_async_file_fd(const swr_v1_os_async_file *);
bool swr_v1_os_async_file_open(swr_v1_os_async_file *, const char *, int, mode_t, int (*effect)(const char *, int, mode_t));
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
ssize_t swr_v1_os_file_read_line(int fd, void *buf, size_t n);
/* src/os/file.cc's std::string helpers and File::read_content; `content` is a
 * swoole::String (default-constructed, or File's fresh one). */
ssize_t swr_v1_os_file_get_size_path(const char *path);
bool swr_v1_os_file_get_contents(const char *path, void *content);
bool swr_v1_os_file_put_contents(const char *path, const void *data, size_t length);
bool swr_v1_os_file_exists(const char *path);
void swr_v1_os_file_read_content(swr_v1_os_file_record *owner, void *content);
/* warn: copy fallback 1 pread, 2 write; kqueue sendfile 1 sendfile. */
typedef struct { ssize_t result; uint32_t warn; } swr_v1_os_sendfile_t;
swr_v1_os_sendfile_t swr_v1_os_sendfile_copy(int out_fd, int in_fd, off_t *offset, size_t size);
#if defined(__linux__) || defined(__ANDROID__) || defined(__APPLE__) || defined(__FreeBSD__)
swr_v1_os_sendfile_t swr_v1_os_sendfile(int out_fd, int in_fd, off_t *offset, size_t size);
#endif
/* src/os/base.cc process and thread identity. Isolation reports each failed step
 * as it happens (errno intact): 1 group lookup, 2 user lookup, 3 setgid,
 * 4 setuid, 5 chdir("/"), 6 chroot. The root check, death signal, thread id
 * and names and swoole_random_bytes are crates/ossupport's under their own symbols. */
/* Darwin daemon steps before fork: 0 done, 1 chdir, 2 open, 3 dup2 failed. */
int swr_v1_os_daemon_detach(bool nochdir, bool noclose);
void swr_v1_os_set_isolation(const char *group, const char *user, const char *root,
                             void (*warn)(uint32_t step, void *context), void *context);
#if defined(__linux__) || defined(__FreeBSD__)
int swr_v1_os_set_cpu_affinity(const void *set);
int swr_v1_os_get_cpu_affinity(void *set);
#endif
bool swr_v1_os_msg_queue_init(swr_v1_os_msg_queue *, key_t, bool, int);
void swr_v1_os_msg_queue_blocking(swr_v1_os_msg_queue *, bool);
bool swr_v1_os_msg_queue_remove(swr_v1_os_msg_queue *);
/* stat: false leaves errno. set_capacity: 0 set, 1 IPC_STAT failed (quiet),
 * 2 IPC_SET failed (warn). */
bool swr_v1_os_msg_queue_stat(const swr_v1_os_msg_queue *, size_t *num, size_t *bytes);
int swr_v1_os_msg_queue_set_capacity(const swr_v1_os_msg_queue *, size_t bytes);
/* SysV message queue. open defaults perms outside (0, 01000) to 0666; push
 * retries EINTR: 0 sent, 1 EAGAIN (quiet), 2 failed (warn); errno intact. */
typedef struct { int id; int perms; } swr_v1_os_msg_open_t;
swr_v1_os_msg_open_t swr_v1_os_msg_open(key_t key, int perms);
int swr_v1_os_msg_flags(int flags, bool blocking);
bool swr_v1_os_msg_remove(int id);
int swr_v1_os_msg_push(int id, const void *msg, size_t length, int flags);
typedef struct { ssize_t result; bool warn; } swr_v1_os_msg_pop_t;
swr_v1_os_msg_pop_t swr_v1_os_msg_pop(int id, void *msg, size_t size, long kind, int flags);
#ifdef __cplusplus
}
static_assert(sizeof(uintptr_t) == sizeof(void *), "opaque token width");
#endif
#endif
