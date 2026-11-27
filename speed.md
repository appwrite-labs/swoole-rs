# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `341aeae` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,719 | 15,210 | 0.9020× | 0.91× | ✅ |
| echo-1k | 16,758 | 18,162 | 0.9227× | 0.92× | ✅ |
| json | 22,051 | 23,173 | 0.9516× | 0.95× | ✅ |
| service-health | 16,308 | 17,782 | 0.9171× | 0.92× | ✅ |
| websocket | 6,736 | 8,192 | 0.8223× | 0.81× | ✅ |
| timer | 202,109 | 313,799 | 0.6441× | 0.65× | ✅ |
| app | 127,559 | 135,966 | 0.9382× | 0.93× | ✅ |
| http2 | 19,172 | 20,496 | 0.9354× | 0.92× | ✅ |
| table | 194,281 | 201,885 | 0.9623× | 0.96× | ✅ |
| channel | 137,019 | 155,726 | 0.8799× | 0.88× | ✅ |
| task | 32,076 | 33,269 | 0.9642× | 0.96× | ✅ |
| mysql | 50,294 | 51,085 | 0.9845× | 0.97× | ✅ |
| http-client | 33,660 | 35,783 | 0.9407× | 0.93× | ✅ |
| gzip | 194,276 | 197,095 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,988 | 181,565 | 0.9913× | 0.99× | ✅ |
| tcp-length | 4,662 | 4,763 | 0.9788× | 0.96× | ✅ |
| tcp-eof | 5,067 | 10,231 | 0.4952× | 0.5× | ✅ |
| udp | 7,616 | 7,987 | 0.9536× | 0.95× | ✅ |
| connect | 17,494 | 18,734 | 0.9338× | 0.93× | ✅ |
| https | 106,288 | 108,806 | 0.9769× | 0.98× | ✅ |
| wss | 101,572 | 104,797 | 0.9692× | 0.97× | ✅ |
| atomic | 50,423 | 51,285 | 0.9832× | 0.97× | ✅ |
| process-mode | 15,407 | 17,021 | 0.9052× | 0.89× | ✅ |
| http2-concurrent | 366,939 | 409,053 | 0.8970× | 0.89× | ✅ |
| websocket-64k | 90,037 | 159,691 | 0.5638× | 0.57× | ✅ |
| websocket-fragmented | 11,963 | 16,368 | 0.7309× | 0.72× | ✅ |
| upload | 418,910 | 1,322,425 | 0.3168× | 0.32× | ✅ |
| static | 25,381 | 41,028 | 0.6186× | 0.62× | ✅ |
| client | 18,598 | 19,730 | 0.9426× | 0.93× | ✅ |
| socket | 19,832 | 21,045 | 0.9423× | 0.93× | ✅ |
| pgsql | 22,894 | 24,663 | 0.9283× | 0.92× | ✅ |
| sqlite | 23,478 | 25,268 | 0.9291× | 0.93× | ✅ |
| curl | 86,069 | 90,420 | 0.9519× | 0.95× | ✅ |
| dns | 230,102 | 232,888 | 0.9880× | 0.99× | ✅ |
| file | 108,880 | 131,300 | 0.8292× | 0.83× | ✅ |

Geometric mean 0.8536×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
