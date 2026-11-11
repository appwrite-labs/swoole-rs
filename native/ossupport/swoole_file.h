/* SPDX-License-Identifier: Apache-2.0 */
// Derived from Swoole v6.2.4 include/swoole_file.h (Swoole contributors): File as the
// compiled logger originals use it. Rust owns its record (crates/ossupport/src/file_record.rs);
// the class only forwards to the swr_v1_os_* calls.
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

}  // namespace swoole
