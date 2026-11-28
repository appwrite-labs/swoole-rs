/* Coroutine file hooks for thirdparty/php/streams/plain_wrapper.c: the libc calls
 * it makes become the hooked swoole_coroutine_* functions (crates/hooks), and it
 * calls flock and socket_create directly. */
#ifndef SW_FILE_HOOK_H_
#define SW_FILE_HOOK_H_
#include <dirent.h>
#include <fcntl.h>
#include <stdio.h>
#include <sys/stat.h>
#include <unistd.h>

int swoole_coroutine_access(const char *pathname, int mode);
int swoole_coroutine_open(const char *pathname, int flags, mode_t mode);
ssize_t swoole_coroutine_read(int fd, void *buf, size_t count);
ssize_t swoole_coroutine_write(int fd, const void *buf, size_t count);
off_t swoole_coroutine_lseek(int fd, off_t offset, int whence);
int swoole_coroutine_unlink(const char *pathname);
int swoole_coroutine_mkdir(const char *pathname, mode_t mode);
int swoole_coroutine_rmdir(const char *pathname);
int swoole_coroutine_rename(const char *oldpath, const char *newpath);
int swoole_coroutine_close(int fd);
int swoole_coroutine_fsync(int fd);
int swoole_coroutine_fdatasync(int fd);
int swoole_coroutine_ftruncate(int fd, off_t length);
FILE *swoole_coroutine_fdopen(int fd, const char *mode);
size_t swoole_coroutine_fread(void *ptr, size_t size, size_t nmemb, FILE *stream);
size_t swoole_coroutine_fwrite(const void *ptr, size_t size, size_t nmemb, FILE *stream);
int swoole_coroutine_fflush(FILE *stream);
int swoole_coroutine_feof(FILE *stream);
int swoole_coroutine_fclose(FILE *stream);
DIR *swoole_coroutine_opendir(const char *name);
struct dirent *swoole_coroutine_readdir(DIR *dirp);
int swoole_coroutine_closedir(DIR *dirp);
int swoole_coroutine_flock(int fd, int operation);
int swoole_coroutine_socket_create(int fd);

#define access(pathname, mode) swoole_coroutine_access(pathname, mode)
#define open(pathname, flags, mode) swoole_coroutine_open(pathname, flags, mode)
#define read(fd, buf, count) swoole_coroutine_read(fd, buf, count)
#define write(fd, buf, count) swoole_coroutine_write(fd, buf, count)
#define lseek(fd, offset, whence) swoole_coroutine_lseek(fd, offset, whence)
#define unlink(pathname) swoole_coroutine_unlink(pathname)
#define mkdir(pathname, mode) swoole_coroutine_mkdir(pathname, mode)
#define rmdir(pathname) swoole_coroutine_rmdir(pathname)
#define rename(oldpath, newpath) swoole_coroutine_rename(oldpath, newpath)
#define close(fd) swoole_coroutine_close(fd)
#define fsync(fd) swoole_coroutine_fsync(fd)
#define fdatasync(fd) swoole_coroutine_fdatasync(fd)
#define ftruncate(fd, length) swoole_coroutine_ftruncate(fd, length)
#define fdopen(fd, mode) swoole_coroutine_fdopen(fd, mode)
#define fread(ptr, size, nmemb, stream) swoole_coroutine_fread(ptr, size, nmemb, stream)
#define fwrite(ptr, size, nmemb, stream) swoole_coroutine_fwrite(ptr, size, nmemb, stream)
#define fflush(stream) swoole_coroutine_fflush(stream)
#define feof(stream) swoole_coroutine_feof(stream)
#define fclose(stream) swoole_coroutine_fclose(stream)
#define opendir(name) swoole_coroutine_opendir(name)
#define readdir(dir) swoole_coroutine_readdir(dir)
#define closedir(dir) swoole_coroutine_closedir(dir)
#endif
