# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `ebf915d` (`benchmarks/callgrind.py`, 1000 requests). Ratio is hybrid ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | hybrid | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,035 | 15,211 | 0.9227× | 0.93× | ✅ |
| echo-1k | 16,951 | 18,155 | 0.9337× | 0.94× | ✅ |
| json | 22,027 | 23,164 | 0.9509× | 0.96× | ✅ |
| service-health | 16,654 | 17,795 | 0.9359× | 0.94× | ✅ |
| websocket | 6,648 | 8,188 | 0.8119× | 0.82× | ✅ |
| timer | 222,952 | 314,352 | 0.7092× | 0.72× | ✅ |
| app | 126,064 | 135,858 | 0.9279× | 0.93× | ✅ |
| http2 | 19,224 | 20,369 | 0.9438× | 0.95× | ✅ |
| table | 198,416 | 201,729 | 0.9836× | 0.99× | ✅ |
| channel | 137,043 | 155,155 | 0.8833× | 0.89× | ✅ |
| task | 31,530 | 33,176 | 0.9504× | 0.96× | ✅ |
| mysql | 49,649 | 51,091 | 0.9718× | 0.98× | ✅ |

Geometric mean 0.9071×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
