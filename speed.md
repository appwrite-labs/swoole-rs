# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `44a4b40` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,793 | 15,202 | 0.9073× | 0.91× | ✅ |
| echo-1k | 16,669 | 18,159 | 0.9179× | 0.92× | ✅ |
| json | 21,739 | 23,164 | 0.9385× | 0.94× | ✅ |
| service-health | 16,391 | 17,796 | 0.9211× | 0.93× | ✅ |
| websocket | 6,603 | 8,191 | 0.8062× | 0.81× | ✅ |
| timer | 222,641 | 314,310 | 0.7083× | 0.72× | ✅ |
| app | 125,105 | 136,065 | 0.9194× | 0.92× | ✅ |
| http2 | 19,286 | 20,343 | 0.9480× | 0.95× | ✅ |
| table | 197,114 | 201,685 | 0.9773× | 0.99× | ✅ |
| channel | 136,485 | 154,568 | 0.8830× | 0.89× | ✅ |
| task | 31,146 | 33,179 | 0.9387× | 0.94× | ✅ |
| mysql | 49,065 | 51,079 | 0.9606× | 0.97× | ✅ |

Geometric mean 0.8990×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
