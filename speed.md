# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `0482774` (`benchmarks/callgrind.py`, 1000 requests). Ratio is hybrid ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | hybrid | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 15,228 | 15,219 | 1.0006× | 1.03× | ➖ |
| echo-1k | 18,145 | 18,136 | 1.0005× | 1.03× | ➖ |
| json | 23,242 | 23,151 | 1.0039× | 1.03× | ➖ |
| service-health | 17,486 | 17,802 | 0.9823× | 1.01× | ✅ |
| websocket | 8,967 | 8,191 | 1.0947× | 1.15× | ➖ |
| timer | 234,775 | 314,139 | 0.7474× | 0.83× | ✅ |
| app | 130,676 | 135,801 | 0.9623× | 1.15× | ✅ |

Geometric mean 0.9646×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/loks0n/swoole-rs/issues/1894)).
