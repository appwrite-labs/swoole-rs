# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `795a4af` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,809 | 15,203 | 0.9083× | 0.91× | ✅ |
| echo-1k | 16,676 | 18,161 | 0.9182× | 0.92× | ✅ |
| json | 21,761 | 23,163 | 0.9395× | 0.94× | ✅ |
| service-health | 16,394 | 17,791 | 0.9215× | 0.93× | ✅ |
| websocket | 6,604 | 8,191 | 0.8062× | 0.81× | ✅ |
| timer | 222,660 | 311,930 | 0.7138× | 0.72× | ✅ |
| app | 124,988 | 135,587 | 0.9218× | 0.92× | ✅ |
| http2 | 19,329 | 20,345 | 0.9501× | 0.95× | ✅ |
| table | 198,136 | 201,681 | 0.9824× | 0.99× | ✅ |
| channel | 136,483 | 154,565 | 0.8830× | 0.89× | ✅ |
| task | 31,122 | 33,172 | 0.9382× | 0.94× | ✅ |
| mysql | 49,058 | 51,076 | 0.9605× | 0.97× | ✅ |

Geometric mean 0.9005×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
