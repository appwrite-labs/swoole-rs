# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `be7677e` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,758 | 15,214 | 0.9043× | 0.91× | ✅ |
| echo-1k | 16,797 | 18,165 | 0.9247× | 0.92× | ✅ |
| json | 22,080 | 23,173 | 0.9528× | 0.95× | ✅ |
| service-health | 16,338 | 17,786 | 0.9186× | 0.92× | ✅ |
| websocket | 6,722 | 8,189 | 0.8209× | 0.81× | ✅ |
| timer | 202,036 | 313,745 | 0.6440× | 0.65× | ✅ |
| app | 127,645 | 135,945 | 0.9390× | 0.93× | ✅ |
| http2 | 18,957 | 20,500 | 0.9247× | 0.92× | ✅ |
| table | 194,306 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,362 | 155,726 | 0.8821× | 0.88× | ✅ |
| task | 32,084 | 33,272 | 0.9643× | 0.96× | ✅ |
| mysql | 50,312 | 51,086 | 0.9848× | 0.97× | ✅ |
| http-client | 33,657 | 35,796 | 0.9403× | 0.93× | ✅ |
| gzip | 194,289 | 197,099 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,163 | 181,573 | 0.9867× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,766 | 0.9779× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,987 | 0.9525× | 0.95× | ✅ |
| connect | 17,533 | 18,734 | 0.9359× | 0.93× | ✅ |
| https | 106,310 | 108,813 | 0.9770× | 0.98× | ✅ |
| wss | 101,595 | 104,803 | 0.9694× | 0.97× | ✅ |
| atomic | 50,193 | 51,285 | 0.9787× | 0.97× | ✅ |
| process-mode | 15,412 | 17,027 | 0.9051× | 0.89× | ✅ |
| http2-concurrent | 364,186 | 409,053 | 0.8903× | 0.89× | ✅ |
| websocket-64k | 90,001 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,927 | 16,368 | 0.7287× | 0.72× | ✅ |
| upload | 416,527 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,941 | 41,063 | 0.6317× | 0.64× | ✅ |
| client | 18,623 | 19,731 | 0.9439× | 0.93× | ✅ |
| socket | 19,860 | 21,045 | 0.9437× | 0.93× | ✅ |
| pgsql | 22,826 | 24,665 | 0.9254× | 0.92× | ✅ |
| sqlite | 23,527 | 25,268 | 0.9311× | 0.93× | ✅ |
| curl | 86,134 | 90,546 | 0.9513× | 0.95× | ✅ |
| dns | 230,146 | 233,710 | 0.9848× | 0.99× | ✅ |
| file | 109,092 | 131,211 | 0.8314× | 0.83× | ✅ |

Geometric mean 0.8535×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
