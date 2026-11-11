/* SPDX-License-Identifier: Apache-2.0 */
// Derived from Swoole v6.2.4 include/swoole_file.h (Swoole contributors): File as the
// ossupport gates use it. Rust owns its record (crates/ossupport/src/file_record.rs);
// the class only forwards to the swr_v1_os_* calls.
#pragma once

#include "swoole_string.h"
#include "swoole_rs_ossupport.h"
#include "swoole_coroutine_api.h"
#include <exception>
#include <new>

#include <sys/file.h>
#include <sys/stat.h>
#include <fcntl.h>

namespace swoole {

inline std::shared_ptr<String> file_get_contents(const std::string &filename) {
    auto content = std::make_shared<String>();
    return swr_v1_os_file_get_contents(filename.c_str(), content.get()) ? content : nullptr;
}

class File {
    // Rust reads owner_ first; the cached native view preserves get_path()'s stable reference API.
    swr_v1_os_file_record *owner_;
    std::string path_;
    struct PathUpdate {
        File *file;
        const std::string *path;
        std::exception_ptr failure;
    };
    static int update_path(void *context) noexcept {
        auto *update = static_cast<PathUpdate *>(context);
        try { update->file->path_ = *update->path; return 0; }
        catch (...) { update->failure = std::current_exception(); return 1; }
    }
    static swr_v1_os_file_record *check(swr_v1_os_file_record *owner) {
        if (!owner) throw std::bad_alloc();
        return owner;
    }
  public:
    enum Flag { READ = O_RDONLY, WRITE = O_WRONLY, RW = O_RDWR, CREATE = O_CREAT, APPEND = O_APPEND };
    explicit File(int fd) : owner_(check(swr_v1_os_file_record_new(fd, nullptr, 0))) {}
    File(const File &other) : owner_(check(swr_v1_os_file_record_clone(other.owner_))), path_(other.path_) {}
    File &operator=(const File &other) {
        if (this != &other) {
            PathUpdate update{this, &other.path_, nullptr};
            int status = swr_v1_os_file_record_assign(owner_, other.owner_, update_path, &update);
            if (update.failure) std::rethrow_exception(update.failure);
            if (status == -2) throw std::bad_alloc();
        }
        return *this;
    }
    File(const std::string &path, int flags) : File(-1) { open(path, flags); }
    File(const std::string &path, int flags, int mode) : File(-1) { open(path, flags, mode); }
    ~File() { swr_v1_os_file_record_free(owner_); }
    bool open(const std::string &path, int flags, int mode = 0) {
        PathUpdate update{this, &path, nullptr};
        int status = swr_v1_os_file_record_open(owner_, path.c_str(), path.size(), flags, mode, update_path, &update);
        if (update.failure) std::rethrow_exception(update.failure);
        if (status == -2) throw std::bad_alloc();
        return status == 1;
    }
    bool close() { return swr_v1_os_file_record_close(owner_); }
    bool ready() const { return get_fd() != -1; }
    ssize_t write(const void *data, size_t n) const {
        return swr_v1_os_file_record_io(owner_, 1, const_cast<void *>(data), n, 0);
    }
    ssize_t write(const std::string &str) const { return write(str.data(), str.size()); }
    ssize_t read(void *data, size_t n) const { return swr_v1_os_file_record_io(owner_, 0, data, n, 0); }
    ssize_t pwrite(const void *data, size_t n, off_t offset) const {
        return swr_v1_os_file_record_io(owner_, 3, const_cast<void *>(data), n, offset);
    }
    ssize_t pread(void *data, size_t n, off_t offset) const {
        return swr_v1_os_file_record_io(owner_, 2, data, n, offset);
    }
    std::shared_ptr<String> read_content() const {
        auto data = std::make_shared<String>();
        swr_v1_os_file_read_content(owner_, data.get());
        return data;
    }
    bool sync() const { return swr_v1_os_file_record_control(owner_, 0, 0) == 0; }
    bool truncate(off_t n) const { return swr_v1_os_file_record_control(owner_, 1, n) == 0; }
    off_t set_offset(off_t n) const { return swr_v1_os_file_record_control(owner_, 2, n); }
    off_t get_offset() const { return swr_v1_os_file_record_control(owner_, 3, 0); }
    bool lock(int operation) const { return swr_v1_os_file_record_control(owner_, 4, operation) == 0; }
    bool unlock() const { return lock(LOCK_UN); }
    void release() { swr_v1_os_file_record_fd(owner_, true); }
    int get_fd() const { return swr_v1_os_file_record_fd(owner_, false); }
    const std::string &get_path() const { return path_; }
    static bool exists(const std::string &file) { return ::access(file.c_str(), R_OK) == 0; }
    static bool remove(const std::string &file) { return ::remove(file.c_str()) == 0; }
};

}  // namespace swoole
