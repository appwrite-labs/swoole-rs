# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `689ca51` (`benchmarks/callgrind.py`, 1000 requests). Ratio is hybrid ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | hybrid | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,045 | 15,227 | 0.9223× | 0.93× | ✅ |
| echo-1k | 16,960 | 18,136 | 0.9352× | 0.94× | ✅ |
| json | 22,049 | 23,151 | 0.9524× | 0.96× | ✅ |
| service-health | 16,656 | 17,802 | 0.9356× | 0.94× | ✅ |
| websocket | 6,650 | 8,188 | 0.8122× | 0.82× | ✅ |
| timer | 222,849 | 314,280 | 0.7091× | 0.72× | ✅ |
| app | 126,130 | 136,071 | 0.9269× | 0.93× | ✅ |
| http2 | 19,233 | 20,369 | 0.9443× | 0.95× | ✅ |
| table | 198,411 | 201,729 | 0.9835× | 0.99× | ✅ |
| channel | 137,042 | 155,155 | 0.8833× | 0.89× | ✅ |
| task | 31,537 | 33,177 | 0.9506× | 0.96× | ✅ |
| mysql | 49,660 | 51,083 | 0.9721× | 0.98× | ✅ |

Geometric mean 0.9073×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/loks0n/swoole-rs/issues/1894)).
