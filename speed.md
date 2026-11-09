# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `2599824` (`benchmarks/callgrind.py`, 1000 requests). Ratio is hybrid ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | hybrid | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,345 | 15,227 | 0.9421× | 1.0× | ✅ |
| echo-1k | 17,269 | 18,145 | 0.9517× | 1.0× | ✅ |
| json | 22,360 | 23,148 | 0.9660× | 1.0× | ✅ |
| service-health | 16,963 | 17,802 | 0.9529× | 0.98× | ✅ |
| websocket | 6,946 | 8,188 | 0.8484× | 0.87× | ✅ |
| timer | 223,563 | 313,061 | 0.7141× | 0.72× | ✅ |
| app | 127,836 | 135,426 | 0.9440× | 0.96× | ✅ |
| http2 | 19,577 | 20,368 | 0.9611× | 1.03× | ✅ |
| table | 199,096 | 201,729 | 0.9869× | 1.0× | ✅ |
| channel | 140,839 | 155,152 | 0.9077× | 0.92× | ✅ |
| task | 33,358 | 33,175 | 1.0055× | 1.03× | ➖ |
| mysql | 50,290 | 51,031 | 0.9855× | 1.1× | ✅ |

Geometric mean 0.9270×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/loks0n/swoole-rs/issues/1894)).
