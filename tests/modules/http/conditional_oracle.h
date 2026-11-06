#pragma once
#include "swoole_server.h"
#include "swoole_http.h"
#include "swoole_rs_http.h"
#include <array>
#include <cassert>
#include <iostream>
#include <random>
#include <sstream>
extern std::vector<std::string> calendar_events;
char *calendar_strptime(const char *,const char *,tm *);
time_t calendar_mktime(tm *);
tm *calendar_gmtime(const time_t *);
#define CONDITIONAL_ORACLE(name) namespace name { \
struct Server { long timezone_ = 0; }; \
class StaticHandler { public: \
 Server *serv = nullptr; time_t file_mtime = 0; \
 time_t get_file_mtime() const { return file_mtime; } \
 bool is_modified(const std::string &) const; \
 bool is_modified_range(const std::string &) const; \
}; }
CONDITIONAL_ORACLE(upstream_conditional)
CONDITIONAL_ORACLE(candidate_conditional)
#undef CONDITIONAL_ORACLE
#define strptime calendar_strptime
#define mktime calendar_mktime
#define gmtime calendar_gmtime

// Preserve the original calendar/errno/call-order probes below the Rust driver.
inline bool conditional_calendar_parse(const char *date,const char *format,void *value) noexcept {
 return calendar_strptime(date,format,static_cast<tm *>(value))!=nullptr;
}
inline int64_t conditional_calendar_time(void *value) noexcept {
 return calendar_mktime(static_cast<tm *>(value));
}
inline void *conditional_calendar_gmt(int64_t value) noexcept {
 const time_t time=value;return calendar_gmtime(&time);
}
inline int32_t conditional_owned_calendar(const char *date,size_t length,uint32_t kind,const int64_t *mtime,const long *timezone,const swr_v3_http_calendar *) {
 const swr_v3_http_calendar calendar{sizeof(tm),alignof(tm),conditional_calendar_parse,conditional_calendar_time,conditional_calendar_gmt};
 return swr_v3_http_conditional(date,length,kind,mtime,timezone,&calendar);
}
#define swr_v3_http_conditional conditional_owned_calendar
