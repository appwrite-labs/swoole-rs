# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `f141fd9` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,106 | 15,202 | 0.9279× | 0.91× | ✅ |
| echo-1k | 17,030 | 18,165 | 0.9375× | 0.92× | ✅ |
| json | 22,115 | 23,164 | 0.9547× | 0.94× | ✅ |
| service-health | 16,722 | 17,799 | 0.9395× | 0.93× | ✅ |
| websocket | 6,562 | 8,185 | 0.8017× | 0.81× | ✅ |
| timer | 211,347 | 314,214 | 0.6726× | 0.68× | ✅ |
| app | 125,250 | 135,917 | 0.9215× | 0.92× | ✅ |
| http2 | 19,486 | 20,339 | 0.9580× | 0.95× | ✅ |
| table | 197,408 | 201,681 | 0.9788× | 0.98× | ✅ |
| channel | 136,487 | 154,568 | 0.8830× | 0.89× | ✅ |
| task | 31,541 | 33,181 | 0.9506× | 0.94× | ✅ |
| mysql | 49,391 | 51,080 | 0.9669× | 0.97× | ✅ |

Geometric mean 0.9033×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
