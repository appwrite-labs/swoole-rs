# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `d1fe81f` (`benchmarks/callgrind.py`, 1000 requests). Ratio is hybrid ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | hybrid | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,817 | 15,206 | 0.9086× | 0.91× | ✅ |
| echo-1k | 16,694 | 18,154 | 0.9196× | 0.92× | ✅ |
| json | 21,760 | 23,163 | 0.9395× | 0.95× | ✅ |
| service-health | 16,402 | 17,791 | 0.9219× | 0.93× | ✅ |
| websocket | 6,577 | 8,188 | 0.8033× | 0.81× | ✅ |
| timer | 226,719 | 314,037 | 0.7220× | 0.73× | ✅ |
| app | 125,456 | 135,965 | 0.9227× | 0.93× | ✅ |
| http2 | 19,278 | 20,341 | 0.9477× | 0.95× | ✅ |
| table | 197,695 | 201,681 | 0.9802× | 0.99× | ✅ |
| channel | 136,481 | 154,568 | 0.8830× | 0.89× | ✅ |
| task | 31,104 | 33,181 | 0.9374× | 0.94× | ✅ |
| mysql | 49,069 | 51,074 | 0.9608× | 0.97× | ✅ |

Geometric mean 0.9010×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
