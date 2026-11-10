# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `1b4765b` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,887 | 15,195 | 0.9139× | 0.91× | ✅ |
| echo-1k | 16,780 | 18,149 | 0.9246× | 0.92× | ✅ |
| json | 21,852 | 23,164 | 0.9433× | 0.94× | ✅ |
| service-health | 16,490 | 17,796 | 0.9266× | 0.93× | ✅ |
| websocket | 6,599 | 8,188 | 0.8059× | 0.81× | ✅ |
| timer | 210,219 | 310,840 | 0.6763× | 0.72× | ✅ |
| app | 125,558 | 135,962 | 0.9235× | 0.92× | ✅ |
| http2 | 19,309 | 20,343 | 0.9492× | 0.95× | ✅ |
| table | 197,205 | 201,681 | 0.9778× | 0.98× | ✅ |
| channel | 136,576 | 154,565 | 0.8836× | 0.89× | ✅ |
| task | 31,233 | 33,177 | 0.9414× | 0.94× | ✅ |
| mysql | 49,150 | 51,078 | 0.9623× | 0.97× | ✅ |

Geometric mean 0.8983×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
