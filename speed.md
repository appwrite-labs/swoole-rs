# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `9759aa0` (`benchmarks/callgrind.py`, 1000 requests). Ratio is hybrid ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | hybrid | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,044 | 15,227 | 0.9223× | 0.93× | ✅ |
| echo-1k | 16,964 | 18,132 | 0.9356× | 0.95× | ✅ |
| json | 22,056 | 23,151 | 0.9527× | 0.96× | ✅ |
| service-health | 16,656 | 17,802 | 0.9356× | 0.95× | ✅ |
| websocket | 6,653 | 8,188 | 0.8125× | 0.82× | ✅ |
| timer | 222,158 | 310,912 | 0.7145× | 0.72× | ✅ |
| app | 125,791 | 136,008 | 0.9249× | 0.94× | ✅ |
| http2 | 19,232 | 20,370 | 0.9441× | 0.96× | ✅ |
| table | 198,411 | 201,726 | 0.9836× | 0.99× | ✅ |
| channel | 137,040 | 155,152 | 0.8833× | 0.89× | ✅ |
| task | 31,549 | 33,179 | 0.9509× | 1.01× | ✅ |
| mysql | 49,607 | 51,083 | 0.9711× | 0.99× | ✅ |

Geometric mean 0.9077×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/loks0n/swoole-rs/issues/1894)).
