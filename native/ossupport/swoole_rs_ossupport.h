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
bool swr_v1_os_file_record_stat(const swr_v1_os_file_record *owner, struct stat *output);
/* File IO. Each call returns right after a failing syscall, errno intact, so
 * native logs the upstream warning. */
typedef struct { size_t done; bool failed; } swr_v1_os_file_io;
swr_v1_os_file_io swr_v1_os_file_record_all(const swr_v1_os_file_record *owner, bool write, void *buffer, size_t length);
int swr_v1_os_file_open(int fd, const char *path, int flags, int mode);
bool swr_v1_os_file_close(int fd);
int swr_v1_os_tmpfile(char *filename);
swr_v1_os_file_io swr_v1_os_file_write_all(int fd, bool append, const void *data, size_t length);
swr_v1_os_file_io swr_v1_os_file_read_all(int fd, void *buf, size_t length);
ssize_t swr_v1_os_file_read_line(int fd, void *buf, size_t n);
/* size, or -1 with the error to publish (errno or EISDIR). */
typedef struct { int64_t size; int error; } swr_v1_os_file_size_t;
swr_v1_os_file_size_t swr_v1_os_file_size(int fd);
swr_v1_os_file_size_t swr_v1_os_file_size_path(const char *path);
/* status 0: fd open for native to read/write and close; otherwise fd is -1:
 * 1 open failed (errno intact), 2 size unknown (publish error), 3 empty,
 * 4 too large. */
typedef struct { int fd; uint32_t status; int64_t size; int error; } swr_v1_os_file_contents_t;
swr_v1_os_file_contents_t swr_v1_os_file_contents_open(const char *path, uint64_t maximum);
swr_v1_os_file_contents_t swr_v1_os_file_put_open(const char *path, uint64_t length, uint64_t maximum);
/* warn: copy fallback 1 pread, 2 write; kqueue sendfile 1 sendfile. */
typedef struct { ssize_t result; uint32_t warn; } swr_v1_os_sendfile_t;
swr_v1_os_sendfile_t swr_v1_os_sendfile_copy(int out_fd, int in_fd, off_t *offset, size_t size);
#if defined(__linux__) || defined(__ANDROID__) || defined(__APPLE__) || defined(__FreeBSD__)
swr_v1_os_sendfile_t swr_v1_os_sendfile(int out_fd, int in_fd, off_t *offset, size_t size);
#endif
size_t swr_v1_os_random_bytes(void *buf, size_t size);
/* src/os/base.cc process and thread identity. Isolation reports each failed step
 * as it happens (errno intact): 1 group lookup, 2 user lookup, 3 setgid,
 * 4 setuid, 5 chdir("/"), 6 chroot. Thread names return the pthread error. */
/* Darwin daemon steps before fork: 0 done, 1 chdir, 2 open, 3 dup2 failed. */
int swr_v1_os_daemon_detach(bool nochdir, bool noclose);
bool swr_v1_os_is_root_user(void);
void swr_v1_os_set_isolation(const char *group, const char *user, const char *root,
                             void (*warn)(uint32_t step, void *context), void *context);
void swr_v1_os_set_process_death_signal(int signal);
#if defined(__linux__) || defined(__FreeBSD__)
int swr_v1_os_set_cpu_affinity(const void *set);
int swr_v1_os_get_cpu_affinity(void *set);
#endif
long swr_v1_os_thread_native_id(void);
int swr_v1_os_thread_set_name(const char *name);
int swr_v1_os_thread_get_name(char *buf, size_t len);
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
