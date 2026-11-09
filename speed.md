# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `d55db90` (`benchmarks/callgrind.py`, 1000 requests). Ratio is hybrid ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | hybrid | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,057 | 15,224 | 0.9233× | 0.93× | ✅ |
| echo-1k | 16,951 | 18,134 | 0.9347× | 0.95× | ✅ |
| json | 22,057 | 23,151 | 0.9528× | 0.96× | ✅ |
| service-health | 16,660 | 17,802 | 0.9359× | 0.95× | ✅ |
| websocket | 6,639 | 8,191 | 0.8105× | 0.82× | ✅ |
| timer | 223,266 | 314,381 | 0.7102× | 0.72× | ✅ |
| app | 125,570 | 135,928 | 0.9238× | 0.94× | ✅ |
| http2 | 19,215 | 20,372 | 0.9432× | 0.96× | ✅ |
| table | 197,915 | 201,729 | 0.9811× | 0.99× | ✅ |
| channel | 137,052 | 155,155 | 0.8833× | 0.89× | ✅ |
| task | 31,501 | 33,186 | 0.9492× | 1.01× | ✅ |
| mysql | 49,597 | 51,084 | 0.9709× | 0.99× | ✅ |

Geometric mean 0.9066×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/loks0n/swoole-rs/issues/1894)).
