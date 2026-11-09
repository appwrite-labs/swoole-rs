# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `793810c` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,809 | 15,245 | 0.9058× | 0.91× | ✅ |
| echo-1k | 16,686 | 18,184 | 0.9176× | 0.92× | ✅ |
| json | 21,770 | 23,198 | 0.9384× | 0.94× | ✅ |
| service-health | 16,394 | 17,830 | 0.9195× | 0.93× | ✅ |
| websocket | 6,604 | 8,230 | 0.8024× | 0.81× | ✅ |
| timer | 222,666 | 311,950 | 0.7138× | 0.72× | ✅ |
| app | 125,272 | 135,783 | 0.9226× | 0.92× | ✅ |
| http2 | 19,340 | 20,381 | 0.9489× | 0.95× | ✅ |
| table | 198,141 | 201,717 | 0.9823× | 0.99× | ✅ |
| channel | 136,487 | 154,647 | 0.8826× | 0.89× | ✅ |
| task | 31,129 | 33,182 | 0.9381× | 0.94× | ✅ |
| mysql | 49,067 | 51,111 | 0.9600× | 0.97× | ✅ |

Geometric mean 0.8996×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
