# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `2c380f6` (`benchmarks/callgrind.py`, 1000 requests). Ratio is hybrid ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | hybrid | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 15,882 | 15,130 | 1.0497× | 1.03× | ❌ |
| echo-1k | 18,855 | 18,074 | 1.0432× | 1.03× | ❌ |
| json | 23,888 | 23,083 | 1.0349× | 1.03× | ❌ |
| service-health | 18,128 | 17,734 | 1.0222× | 1.01× | ❌ |
| websocket | 9,574 | 8,188 | 1.1694× | 1.15× | ❌ |
| timer | 262,049 | 311,770 | 0.8405× | 0.83× | ✅ |

Geometric mean 1.0219×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/loks0n/swoole-rs/issues/1894)).
