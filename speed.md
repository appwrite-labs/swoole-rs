# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `015ac0a` (`benchmarks/callgrind.py`, 1000 requests). Ratio is hybrid ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | hybrid | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,052 | 15,227 | 0.9228× | 0.93× | ✅ |
| echo-1k | 16,960 | 18,136 | 0.9352× | 0.94× | ✅ |
| json | 22,053 | 23,151 | 0.9526× | 0.96× | ✅ |
| service-health | 16,656 | 17,802 | 0.9356× | 0.94× | ✅ |
| websocket | 6,649 | 8,188 | 0.8121× | 0.82× | ✅ |
| timer | 222,407 | 310,475 | 0.7163× | 0.72× | ✅ |
| app | 126,059 | 135,811 | 0.9282× | 0.93× | ✅ |
| http2 | 19,237 | 20,368 | 0.9445× | 0.95× | ✅ |
| table | 198,415 | 201,729 | 0.9836× | 0.99× | ✅ |
| channel | 137,043 | 155,156 | 0.8833× | 0.89× | ✅ |
| task | 31,548 | 33,182 | 0.9508× | 0.96× | ✅ |
| mysql | 49,667 | 51,083 | 0.9723× | 0.98× | ✅ |

Geometric mean 0.9082×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/loks0n/swoole-rs/issues/1894)).
