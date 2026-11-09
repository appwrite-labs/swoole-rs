# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `d9d9897` (`benchmarks/callgrind.py`, 1000 requests). Ratio is hybrid ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | hybrid | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,825 | 15,211 | 0.9089× | 0.93× | ✅ |
| echo-1k | 16,671 | 18,150 | 0.9185× | 0.94× | ✅ |
| json | 21,771 | 23,164 | 0.9399× | 0.96× | ✅ |
| service-health | 16,415 | 17,795 | 0.9224× | 0.94× | ✅ |
| websocket | 6,574 | 8,191 | 0.8025× | 0.82× | ✅ |
| timer | 211,249 | 313,259 | 0.6744× | 0.71× | ✅ |
| app | 125,427 | 136,031 | 0.9220× | 0.93× | ✅ |
| http2 | 19,268 | 20,368 | 0.9460× | 0.95× | ✅ |
| table | 197,751 | 201,729 | 0.9803× | 0.99× | ✅ |
| channel | 136,560 | 155,155 | 0.8801× | 0.89× | ✅ |
| task | 31,098 | 33,176 | 0.9374× | 0.96× | ✅ |
| mysql | 49,068 | 51,087 | 0.9605× | 0.98× | ✅ |

Geometric mean 0.8954×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
