# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `bd8ecf3` (`benchmarks/callgrind.py`, 1000 requests). Ratio is hybrid ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | hybrid | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,048 | 15,227 | 0.9225× | 0.93× | ✅ |
| echo-1k | 16,965 | 18,147 | 0.9348× | 0.95× | ✅ |
| json | 22,057 | 23,151 | 0.9527× | 0.96× | ✅ |
| service-health | 16,656 | 17,802 | 0.9356× | 0.95× | ✅ |
| websocket | 6,652 | 8,188 | 0.8124× | 0.82× | ✅ |
| timer | 223,049 | 314,603 | 0.7090× | 0.72× | ✅ |
| app | 125,796 | 135,914 | 0.9256× | 0.94× | ✅ |
| http2 | 19,236 | 20,368 | 0.9444× | 0.96× | ✅ |
| table | 198,415 | 201,729 | 0.9836× | 0.99× | ✅ |
| channel | 137,043 | 155,155 | 0.8833× | 0.89× | ✅ |
| task | 31,555 | 33,188 | 0.9508× | 1.01× | ✅ |
| mysql | 49,611 | 51,087 | 0.9711× | 0.99× | ✅ |

Geometric mean 0.9071×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/loks0n/swoole-rs/issues/1894)).
