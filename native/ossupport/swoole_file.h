/* SPDX-License-Identifier: Apache-2.0 */
// Derived from Swoole v6.2.4 include/swoole_file.h (Swoole contributors): File and AsyncFile
// as the compiled logger originals and the ossupport gate use them. Rust owns both
// records (crates/ossupport/src/file_record.rs and async_file.rs); these classes only
// forward to the swr_v1_os_* calls.
#pragma once

#include "swoole_string.h"
#include "swoole_rs_ossupport.h"
#include "swoole_coroutine_api.h"
#include <sys/file.h>
#include <sys/stat.h>
#include <fcntl.h>

namespace swoole {

inline std::shared_ptr<String> file_get_contents(const std::string &filename) {
    auto content = std::make_shared<String>();
    return swr_v1_os_file_get_contents(filename.c_str(), content.get()) ? content : nullptr;
}

// Only what the logger originals call: open, read_content and the destructor.
class File {
    swr_v1_os_file_record *owner_;
  public:
    enum Flag { READ = O_RDONLY, WRITE = O_WRONLY, RW = O_RDWR, CREATE = O_CREAT, APPEND = O_APPEND };
    File(const std::string &path, int flags, int mode = 0) : owner_(swr_v1_os_file_record_open(path.c_str(), flags, mode)) {}
    File(const File &) = delete;
    File &operator=(const File &) = delete;
    ~File() { swr_v1_os_file_record_free(owner_); }
    std::shared_ptr<String> read_content() const {
        auto data = std::make_shared<String>();
        swr_v1_os_file_read_content(owner_, data.get());
        return data;
    }
};

class AsyncFileOwner {
  protected:
    swr_v1_os_async_file *owner_;
    explicit AsyncFileOwner(swr_v1_os_async_file *owner) : owner_(owner) {}
    AsyncFileOwner(const AsyncFileOwner &) = delete;
    AsyncFileOwner &operator=(const AsyncFileOwner &) = delete;
    ~AsyncFileOwner() { swr_v1_os_async_file_free(owner_); }
};
class AsyncFile : private AsyncFileOwner {
  public:
    AsyncFile(const std::string &path, int flags, int mode)
        : AsyncFileOwner(swr_v1_os_async_file_new()) { open(path, flags, mode); }
    AsyncFile(const AsyncFile &source)
        : AsyncFileOwner(swr_v1_os_async_file_clone(source.owner_)) {}
    AsyncFile &operator=(const AsyncFile &source) {
        swr_v1_os_async_file_assign(owner_, source.owner_);
        return *this;
    }
    ~AsyncFile() { close(); }
    bool open(const std::string &path, int flags, mode_t mode) {
        close();
        return swr_v1_os_async_file_open(owner_, path.c_str(), flags, mode, swoole_coroutine_open);
    }
    // Hooked close can destroy opaque native payloads; finish it outside Rust
    // before AsyncFileOwner releases the record.
    bool close() const {
        int fd = swr_v1_os_async_file_fd(owner_);
        return fd != -1 && swoole_coroutine_close(fd) == 0;
    }
    ssize_t read(void *buf, size_t count) const { return swr_v1_os_async_file_read(owner_, buf, count, swoole_coroutine_read); }
    ssize_t write(const void *buf, size_t count) const { return swr_v1_os_async_file_write(owner_, buf, count, swoole_coroutine_write); }
    bool sync() const { return swr_v1_os_async_file_sync(owner_, swoole_coroutine_fsync); }
    bool truncate(off_t length) const { return swr_v1_os_async_file_truncate(owner_, length, swoole_coroutine_ftruncate); }
    bool stat(struct stat *statbuf) const { return swr_v1_os_async_file_stat(owner_, statbuf, swoole_coroutine_fstat); }
    off_t get_offset() const { return swr_v1_os_async_file_offset(owner_, 0, SEEK_CUR, swoole_coroutine_lseek); }
    off_t set_offset(off_t offset) const { return swr_v1_os_async_file_offset(owner_, offset, SEEK_SET, swoole_coroutine_lseek); }
    bool ready() const { return swr_v1_os_async_file_fd(owner_) != -1; }
};

}  // namespace swoole
