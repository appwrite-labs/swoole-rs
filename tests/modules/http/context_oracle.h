#pragma once
#include "swoole_static_handler.h"
#include "swoole_rs_http_context.h"
#include "index_buffer.h"
#include <dirent.h>
#include <regex>
#include <iostream>
#include <unordered_map>
#define CONTEXT_EFFECTS(ns) namespace ns { \
using String=IndexBuffer;using RewriteRule=swoole::http_server::RewriteRule; \
inline std::vector<std::string> names,trace;inline std::string mime="text/plain"; \
inline size_t file_size=100;inline time_t file_time=1700000000;inline std::string directory_name,link_name,canonical_name; \
struct DIR {size_t next=0;struct dirent entry{};}; \
inline DIR *opendir(const char *filename){trace.emplace_back(std::string("open:")+filename);return new DIR();} \
inline struct dirent *readdir(DIR *dir){if(dir->next==names.size()){trace.emplace_back("end");return nullptr;}auto &name=names[dir->next++];trace.emplace_back("read:"+name);memcpy(dir->entry.d_name,name.data(),name.size());dir->entry.d_name[name.size()]=0;return &dir->entry;} \
inline int closedir(DIR *dir){trace.emplace_back("close");delete dir;return 0;} \
inline int lstat(const char *path,struct stat *value){trace.emplace_back(std::string("stat:")+path);memset(value,0,sizeof(*value));value->st_size=file_size;value->st_mode=link_name==path?S_IFLNK:directory_name==path?S_IFDIR:S_IFREG;set_mtime(value,file_time);return 0;} \
inline char *realpath(const char *input,char *output){trace.emplace_back(std::string("canonical:")+input);strcpy(output,canonical_name.empty()?input:canonical_name.c_str());return output;} \
inline void swoole_random_string(std::string &value,size_t count){trace.emplace_back("random:"+std::to_string(count));value.append(count,'x');} \
inline void swoole_random_string(char *value,size_t count){trace.emplace_back("random:"+std::to_string(count));memset(value,'x',count);value[count]=0;} \
namespace mime_type {inline bool exists(const std::string &path){trace.emplace_back("mime-exists:"+path);return true;} \
inline const std::string &get(const std::string &path){trace.emplace_back("mime:"+path);return mime;}} \
}
inline void set_mtime(struct stat *value,time_t mtime){
#ifdef __MACH__
 value->st_mtimespec.tv_sec=mtime;
#else
 value->st_mtim.tv_sec=mtime;
#endif
}
CONTEXT_EFFECTS(upstream_context)
CONTEXT_EFFECTS(candidate_context)
#undef CONTEXT_EFFECTS

namespace candidate_context {
// The candidate looks MIME types up in Rust's table; trace it as the original's mime_type calls.
inline const char *swr_v1_framing_mime_get(const uint8_t *path, size_t length) {
    trace.emplace_back("mime:" + std::string(reinterpret_cast<const char *>(path), length));
    return mime.c_str();
}
inline bool swr_v1_framing_mime_exists(const uint8_t *path, size_t length) {
    trace.emplace_back("mime-exists:" + std::string(reinterpret_cast<const char *>(path), length));
    return true;
}
inline bool static_io_stat(const char *path, swr_v2_http_observation *out) noexcept {
    struct stat value{};
    if (candidate_context::lstat(path, &value) != 0) return false;
    out->filesize = value.st_size;
#ifdef __MACH__
    out->mtime = value.st_mtimespec.tv_sec;
#else
    out->mtime = value.st_mtim.tv_sec;
#endif
    out->kind = (S_ISDIR(value.st_mode) ? SWR_HTTP_PATH_IS_DIRECTORY : 0u) |
        (S_ISREG(value.st_mode) ? SWR_HTTP_PATH_FILE : 0u) |
        (S_ISLNK(value.st_mode) ? SWR_HTTP_PATH_LINK : 0u);
    return true;
}
inline char *static_io_canonical(const char *path, char *out) noexcept { return candidate_context::realpath(path, out); }
inline void *static_io_open(const char *path) noexcept { return candidate_context::opendir(path); }
inline const char *static_io_read(void *opaque) noexcept {
    auto *entry = candidate_context::readdir(static_cast<candidate_context::DIR *>(opaque));
    return entry ? entry->d_name : nullptr;
}
inline void static_io_close(void *opaque) noexcept { candidate_context::closedir(static_cast<candidate_context::DIR *>(opaque)); }
inline const swr_v3_http_static_io static_io{static_io_stat,static_io_canonical,static_io_open,static_io_read,static_io_close};
inline int32_t static_io_stat_context(swr_v2_http_context *context, const swr_v3_http_static_io *) {
    return swr_v3_http_context_stat(context, &static_io);
}
inline int32_t static_io_directory(swr_v2_http_context *context, const swr_v3_http_static_io *) {
    return swr_v3_http_context_directory(context, &static_io);
}
inline int32_t static_io_serve(swr_v2_http_context *context, const swr_v1_http_proxy_text *root,
    const swr_v2_http_serve_observation *observation, const swr_v3_http_static_io *) {
    return swr_v3_http_context_serve(context, root, observation, &static_io);
}
}
#define swr_v3_http_context_stat candidate_context::static_io_stat_context
#define swr_v3_http_context_directory candidate_context::static_io_directory
#define swr_v3_http_context_serve candidate_context::static_io_serve
