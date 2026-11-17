# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `b93a7de` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,788 | 15,214 | 0.9063× | 0.91× | ✅ |
| echo-1k | 16,808 | 18,165 | 0.9253× | 0.93× | ✅ |
| json | 21,951 | 23,161 | 0.9478× | 0.95× | ✅ |
| service-health | 16,360 | 17,777 | 0.9203× | 0.92× | ✅ |
| websocket | 6,547 | 8,188 | 0.7996× | 0.8× | ✅ |
| timer | 205,518 | 313,776 | 0.6550× | 0.65× | ✅ |
| app | 126,814 | 135,972 | 0.9326× | 0.93× | ✅ |
| http2 | 18,994 | 20,468 | 0.9280× | 0.92× | ✅ |
| table | 194,812 | 201,865 | 0.9651× | 0.97× | ✅ |
| channel | 137,570 | 155,170 | 0.8866× | 0.88× | ✅ |
| task | 32,495 | 33,264 | 0.9769× | 0.96× | ✅ |
| mysql | 49,931 | 51,052 | 0.9780× | 0.97× | ✅ |
| http-client | 33,622 | 35,803 | 0.9391× | 0.93× | ✅ |
| gzip | 194,460 | 197,079 | 0.9867× | 0.99× | ✅ |
| websocket-deflate | 178,954 | 181,568 | 0.9856× | 0.99× | ✅ |
| tcp-length | 4,584 | 4,762 | 0.9626× | 0.96× | ✅ |
| tcp-eof | 5,164 | 10,231 | 0.5047× | 0.5× | ✅ |
| udp | 7,485 | 7,987 | 0.9372× | 0.93× | ✅ |
| connect | 17,490 | 18,707 | 0.9350× | 0.93× | ✅ |
| https | 105,953 | 108,803 | 0.9738× | 0.98× | ✅ |
| wss | 100,985 | 104,794 | 0.9637× | 0.97× | ✅ |
| atomic | 49,859 | 51,316 | 0.9716× | 0.97× | ✅ |
| process-mode | 15,048 | 17,027 | 0.8837× | 0.88× | ✅ |
| http2-concurrent | 363,429 | 409,069 | 0.8884× | 0.88× | ✅ |
| websocket-64k | 89,802 | 159,692 | 0.5623× | 0.57× | ✅ |
| websocket-fragmented | 11,644 | 16,328 | 0.7132× | 0.72× | ✅ |
| upload | 415,233 | 1,322,387 | 0.3140× | 0.32× | ✅ |
| static | 26,649 | 41,051 | 0.6492× | 0.65× | ✅ |
| client | 18,561 | 19,758 | 0.9395× | 0.94× | ✅ |
| socket | 19,702 | 21,072 | 0.9350× | 0.92× | ✅ |
| pgsql | 22,759 | 24,595 | 0.9253× | 0.93× | ✅ |
| sqlite | 23,478 | 25,221 | 0.9309× | 0.93× | ✅ |
| curl | 86,425 | 90,320 | 0.9569× | 0.94× | ✅ |
| dns | 230,249 | 232,667 | 0.9896× | 0.99× | ✅ |

Geometric mean 0.8526×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
