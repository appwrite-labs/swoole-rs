# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `856f62c` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,755 | 15,214 | 0.9041× | 0.91× | ✅ |
| echo-1k | 16,797 | 18,165 | 0.9247× | 0.92× | ✅ |
| json | 22,084 | 23,164 | 0.9533× | 0.95× | ✅ |
| service-health | 16,341 | 17,777 | 0.9192× | 0.92× | ✅ |
| websocket | 6,725 | 8,189 | 0.8213× | 0.81× | ✅ |
| timer | 201,925 | 314,077 | 0.6429× | 0.65× | ✅ |
| app | 127,625 | 135,914 | 0.9390× | 0.93× | ✅ |
| http2 | 18,968 | 20,501 | 0.9252× | 0.92× | ✅ |
| table | 194,374 | 202,900 | 0.9580× | 0.96× | ✅ |
| channel | 137,362 | 155,714 | 0.8821× | 0.88× | ✅ |
| task | 32,088 | 33,272 | 0.9644× | 0.96× | ✅ |
| mysql | 50,308 | 51,088 | 0.9847× | 0.97× | ✅ |
| http-client | 33,633 | 35,783 | 0.9399× | 0.93× | ✅ |
| gzip | 194,275 | 197,095 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,562 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,766 | 0.9779× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,987 | 0.9525× | 0.95× | ✅ |
| connect | 17,532 | 18,734 | 0.9359× | 0.93× | ✅ |
| https | 106,310 | 108,809 | 0.9770× | 0.98× | ✅ |
| wss | 101,597 | 104,800 | 0.9694× | 0.97× | ✅ |
| atomic | 50,196 | 51,276 | 0.9789× | 0.97× | ✅ |
| process-mode | 15,412 | 17,024 | 0.9053× | 0.89× | ✅ |
| http2-concurrent | 364,384 | 408,861 | 0.8912× | 0.89× | ✅ |
| websocket-64k | 89,999 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,927 | 16,368 | 0.7286× | 0.72× | ✅ |
| upload | 416,528 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,912 | 41,091 | 0.6306× | 0.64× | ✅ |
| client | 18,621 | 19,720 | 0.9442× | 0.93× | ✅ |
| socket | 19,860 | 21,033 | 0.9442× | 0.93× | ✅ |
| pgsql | 22,825 | 24,664 | 0.9255× | 0.92× | ✅ |
| sqlite | 23,527 | 25,268 | 0.9311× | 0.93× | ✅ |
| curl | 86,130 | 90,520 | 0.9515× | 0.95× | ✅ |
| dns | 229,964 | 233,246 | 0.9859× | 0.99× | ✅ |
| file | 109,092 | 131,300 | 0.8309× | 0.83× | ✅ |

Geometric mean 0.8534×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
