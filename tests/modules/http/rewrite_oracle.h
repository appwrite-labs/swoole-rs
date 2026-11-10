#pragma once
#include "swoole.h"
#include "swoole_util.h"
#include <memory>
#include <regex>
#include <iostream>
#include <random>
#include <string>
#include <vector>
// The pinned add_rewrite_rule/apply_rewrite_rules, extracted into upstream_rewrite, over a
// handler that publishes into native strings.
namespace upstream_rewrite {
namespace http_server {
struct RewriteRule{std::string pattern,replacement;bool is_regex=false;};
struct StaticHandler {std::string url,original;unsigned publications=0;explicit StaticHandler(const std::string &value):url(value),original(value){}const std::string &get_request_url()const{return url;}void set_request_url(const std::string &value){++publications;url=value;} };
}
struct Server {std::shared_ptr<std::vector<http_server::RewriteRule>> rewrite_rules;void add_rewrite_rule(const std::string &,const std::string &);bool apply_rewrite_rules(http_server::StaticHandler *);};
}
