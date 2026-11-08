# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `5cf20bb` (`benchmarks/callgrind.py`, 1000 requests). Ratio is hybrid ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | hybrid | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 15,882 | 15,133 | 1.0495× | 1.03× | ❌ |
| echo-1k | 18,838 | 18,073 | 1.0423× | 1.03× | ❌ |
| json | 23,892 | 23,083 | 1.0350× | 1.03× | ❌ |
| service-health | 18,128 | 17,734 | 1.0222× | 1.01× | ❌ |
| websocket | 9,571 | 8,191 | 1.1685× | 1.15× | ❌ |
| timer | 263,508 | 314,580 | 0.8376× | 0.83× | ✅ |

Geometric mean 1.0210×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/loks0n/swoole-rs/issues/1894)).
