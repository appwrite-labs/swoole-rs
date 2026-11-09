# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `9abf3dc` (`benchmarks/callgrind.py`, 1000 requests). Ratio is hybrid ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | hybrid | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,112 | 15,203 | 0.9283× | 0.93× | ✅ |
| echo-1k | 17,016 | 18,163 | 0.9369× | 0.94× | ✅ |
| json | 22,104 | 23,163 | 0.9543× | 0.96× | ✅ |
| service-health | 16,713 | 17,795 | 0.9392× | 0.94× | ✅ |
| websocket | 6,719 | 8,188 | 0.8206× | 0.82× | ✅ |
| timer | 220,990 | 311,212 | 0.7101× | 0.71× | ✅ |
| app | 126,458 | 135,956 | 0.9301× | 0.93× | ✅ |
| http2 | 19,366 | 20,343 | 0.9520× | 0.95× | ✅ |
| table | 198,453 | 201,678 | 0.9840× | 0.99× | ✅ |
| channel | 137,060 | 154,565 | 0.8867× | 0.89× | ✅ |
| task | 31,590 | 33,180 | 0.9521× | 0.96× | ✅ |
| mysql | 49,806 | 51,072 | 0.9752× | 0.98× | ✅ |

Geometric mean 0.9108×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
