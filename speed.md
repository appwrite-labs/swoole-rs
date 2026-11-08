# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `bbb0566` (`benchmarks/callgrind.py`, 1000 requests). Ratio is hybrid ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | hybrid | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 15,500 | 15,133 | 1.0242× | 1.03× | ➖ |
| echo-1k | 18,468 | 18,072 | 1.0219× | 1.03× | ➖ |
| json | 23,527 | 23,083 | 1.0192× | 1.03× | ➖ |
| service-health | 17,773 | 17,730 | 1.0024× | 1.01× | ➖ |
| websocket | 9,267 | 8,189 | 1.1317× | 1.15× | ➖ |
| timer | 256,695 | 313,238 | 0.8195× | 0.83× | ✅ |

Geometric mean 0.9986×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/loks0n/swoole-rs/issues/1894)).
