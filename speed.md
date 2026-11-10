# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `eb08426` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,118 | 15,198 | 0.9289× | 0.91× | ✅ |
| echo-1k | 17,010 | 18,151 | 0.9372× | 0.92× | ✅ |
| json | 22,104 | 23,164 | 0.9542× | 0.94× | ✅ |
| service-health | 16,721 | 17,799 | 0.9394× | 0.93× | ✅ |
| websocket | 6,558 | 8,184 | 0.8013× | 0.81× | ✅ |
| timer | 211,439 | 314,291 | 0.6727× | 0.68× | ✅ |
| app | 125,249 | 135,998 | 0.9210× | 0.92× | ✅ |
| http2 | 19,485 | 20,339 | 0.9580× | 0.95× | ✅ |
| table | 197,403 | 201,681 | 0.9788× | 0.98× | ✅ |
| channel | 136,485 | 154,568 | 0.8830× | 0.89× | ✅ |
| task | 31,496 | 33,182 | 0.9492× | 0.94× | ✅ |
| mysql | 49,368 | 51,080 | 0.9665× | 0.97× | ✅ |

Geometric mean 0.9031×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
