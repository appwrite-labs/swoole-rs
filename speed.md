# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `395e4d7` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,737 | 15,214 | 0.9029× | 0.91× | ✅ |
| echo-1k | 16,728 | 18,165 | 0.9209× | 0.92× | ✅ |
| json | 22,066 | 23,173 | 0.9522× | 0.95× | ✅ |
| service-health | 16,313 | 17,786 | 0.9172× | 0.92× | ✅ |
| websocket | 6,676 | 8,189 | 0.8153× | 0.8× | ✅ |
| timer | 201,357 | 314,036 | 0.6412× | 0.65× | ✅ |
| app | 126,706 | 135,892 | 0.9324× | 0.93× | ✅ |
| http2 | 18,977 | 20,499 | 0.9258× | 0.92× | ✅ |
| table | 193,905 | 201,885 | 0.9605× | 0.96× | ✅ |
| channel | 138,487 | 155,726 | 0.8893× | 0.88× | ✅ |
| task | 31,931 | 33,269 | 0.9598× | 0.96× | ✅ |
| mysql | 49,837 | 51,061 | 0.9760× | 0.97× | ✅ |
| http-client | 33,624 | 35,787 | 0.9396× | 0.93× | ✅ |
| gzip | 194,338 | 197,099 | 0.9860× | 0.99× | ✅ |
| websocket-deflate | 179,107 | 181,565 | 0.9865× | 0.99× | ✅ |
| tcp-length | 4,644 | 4,763 | 0.9750× | 0.96× | ✅ |
| tcp-eof | 5,194 | 10,228 | 0.5078× | 0.5× | ✅ |
| udp | 7,579 | 7,987 | 0.9490× | 0.95× | ✅ |
| connect | 17,572 | 18,734 | 0.9380× | 0.93× | ✅ |
| https | 106,024 | 108,809 | 0.9744× | 0.98× | ✅ |
| wss | 101,239 | 104,800 | 0.9660× | 0.97× | ✅ |
| atomic | 50,172 | 51,285 | 0.9783× | 0.97× | ✅ |
| process-mode | 15,255 | 17,027 | 0.8959× | 0.89× | ✅ |
| http2-concurrent | 364,787 | 409,057 | 0.8918× | 0.89× | ✅ |
| websocket-64k | 89,974 | 159,694 | 0.5634× | 0.57× | ✅ |
| websocket-fragmented | 11,844 | 16,372 | 0.7234× | 0.72× | ✅ |
| upload | 416,224 | 1,322,429 | 0.3147× | 0.32× | ✅ |
| static | 27,103 | 41,063 | 0.6600× | 0.65× | ✅ |
| client | 18,386 | 19,729 | 0.9319× | 0.93× | ✅ |
| socket | 19,639 | 21,045 | 0.9332× | 0.93× | ✅ |
| pgsql | 22,587 | 24,618 | 0.9175× | 0.92× | ✅ |
| sqlite | 23,401 | 25,236 | 0.9273× | 0.93× | ✅ |
| curl | 85,345 | 90,359 | 0.9445× | 0.95× | ✅ |
| dns | 230,084 | 232,975 | 0.9876× | 0.99× | ✅ |

Geometric mean 0.8533×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
