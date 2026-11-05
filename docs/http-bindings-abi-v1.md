# PHP HTTP bindings boundary (M23)

Feature value 2097152. Upstream targets:
`ext-src/swoole_http_{cookie,request,response,server,server_coro}.cc`.
`php_swoole_http.h`, `php_swoole_http_server.h`, stubs/arginfo and class
declarations are unchanged. Follows the
[policy/effect split](abi-v1.md#policyeffect-split). HTTP framing and parsing
reuse M04; there is no second parser.

Zend parameter parsing, object creation, HashTable/property mutation, refcounts,
GC, COW/reference identity, warnings and callbacks are native. Object,
HttpContext, session and connection stores are the only storage. A caller-local
phase cursor may survive native effects; nothing requiring a heap destructor is
left to clean up after a bailout. Partial mutations, exceptions, warnings,
last-error and early returns match upstream.

- **Cookie**: PHP RFC 1738 encoding (`+` for space, escaped `~`), which differs
  from M04's URL encoding. `smart_str` partial-append/warning order and
  user error-handler reentry are kept.
- **Request headers**: llhttp delivers headers in fragments. Kept:
  `current_header` overwrite, per-fragment `treat_data` cookie parsing,
  known-name overwrite versus unknown-name merge, `parse_cookie` early return,
  upgrade flag published before connection lookup. The original
  `create_request` assertion fails on 67 of 501 split points on both upstream
  and hybrid.
- `getContent` precedence (body/chunked/HTTP2/empty), `getData` warnings and
  string references, and Request option iteration match upstream.
- **Response destruction**: `onAfterResponse`, then conditional fallback
  500/`end`, clear `response.zobject`, free context, then the Zend destructor.

Original inventory: 194 HTTP PHPTs plus 22 conditional HTTP/2 server PHPTs.
See also [suspended response lifetime](http-response-lifetime.md).
