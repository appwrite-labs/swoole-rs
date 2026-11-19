# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `22f5f5e` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,740 | 15,214 | 0.9031× | 0.91× | ✅ |
| echo-1k | 16,752 | 18,162 | 0.9224× | 0.92× | ✅ |
| json | 22,077 | 23,161 | 0.9532× | 0.95× | ✅ |
| service-health | 16,335 | 17,777 | 0.9189× | 0.92× | ✅ |
| websocket | 6,676 | 8,192 | 0.8149× | 0.81× | ✅ |
| timer | 201,455 | 313,778 | 0.6420× | 0.65× | ✅ |
| app | 126,980 | 135,886 | 0.9345× | 0.93× | ✅ |
| http2 | 18,957 | 20,497 | 0.9248× | 0.92× | ✅ |
| table | 193,854 | 202,900 | 0.9554× | 0.96× | ✅ |
| channel | 137,358 | 155,717 | 0.8821× | 0.88× | ✅ |
| task | 31,957 | 33,265 | 0.9607× | 0.96× | ✅ |
| mysql | 50,046 | 51,085 | 0.9797× | 0.97× | ✅ |
| http-client | 33,367 | 35,787 | 0.9324× | 0.93× | ✅ |
| gzip | 194,374 | 197,099 | 0.9862× | 0.99× | ✅ |
| websocket-deflate | 179,107 | 181,565 | 0.9865× | 0.99× | ✅ |
| tcp-length | 4,646 | 4,766 | 0.9748× | 0.96× | ✅ |
| tcp-eof | 5,196 | 10,231 | 0.5079× | 0.5× | ✅ |
| udp | 7,561 | 7,986 | 0.9467× | 0.95× | ✅ |
| connect | 17,580 | 18,730 | 0.9386× | 0.93× | ✅ |
| https | 106,000 | 108,813 | 0.9741× | 0.98× | ✅ |
| wss | 101,212 | 104,800 | 0.9658× | 0.97× | ✅ |
| atomic | 50,179 | 51,285 | 0.9784× | 0.97× | ✅ |
| process-mode | 15,262 | 17,021 | 0.8967× | 0.89× | ✅ |
| http2-concurrent | 366,298 | 409,057 | 0.8955× | 0.89× | ✅ |
| websocket-64k | 89,970 | 159,694 | 0.5634× | 0.57× | ✅ |
| websocket-fragmented | 11,839 | 16,368 | 0.7233× | 0.72× | ✅ |
| upload | 416,427 | 1,322,425 | 0.3149× | 0.32× | ✅ |
| static | 27,051 | 40,999 | 0.6598× | 0.65× | ✅ |
| client | 18,468 | 19,730 | 0.9360× | 0.93× | ✅ |
| socket | 19,693 | 21,046 | 0.9357× | 0.93× | ✅ |
| pgsql | 23,104 | 24,662 | 0.9368× | 0.92× | ✅ |
| sqlite | 23,449 | 25,268 | 0.9280× | 0.93× | ✅ |
| curl | 85,939 | 90,422 | 0.9504× | 0.95× | ✅ |
| dns | 229,519 | 232,775 | 0.9860× | 0.99× | ✅ |
| file | 108,213 | 131,300 | 0.8242× | 0.83× | ✅ |

Geometric mean 0.8531×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
