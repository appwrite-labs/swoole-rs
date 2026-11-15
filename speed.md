# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `df54a73` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,959 | 15,210 | 0.9178× | 0.92× | ✅ |
| echo-1k | 17,046 | 18,165 | 0.9384× | 0.94× | ✅ |
| json | 22,093 | 23,161 | 0.9539× | 0.95× | ✅ |
| service-health | 16,534 | 17,777 | 0.9301× | 0.93× | ✅ |
| websocket | 6,618 | 8,188 | 0.8083× | 0.8× | ✅ |
| timer | 205,863 | 313,534 | 0.6566× | 0.65× | ✅ |
| app | 125,288 | 135,949 | 0.9216× | 0.93× | ✅ |
| http2 | 18,750 | 20,469 | 0.9160× | 0.91× | ✅ |
| table | 195,621 | 201,865 | 0.9691× | 0.97× | ✅ |
| channel | 136,409 | 155,170 | 0.8791× | 0.88× | ✅ |
| task | 32,506 | 33,265 | 0.9772× | 0.96× | ✅ |
| mysql | 49,550 | 51,061 | 0.9704× | 0.97× | ✅ |
| http-client | 33,806 | 35,803 | 0.9442× | 0.94× | ✅ |
| client | 18,528 | 19,756 | 0.9378× | 0.94× | ✅ |
| socket | 19,645 | 21,072 | 0.9323× | 0.92× | ✅ |
| gzip | 194,157 | 197,079 | 0.9852× | 0.99× | ✅ |
| websocket-deflate | 179,075 | 181,572 | 0.9862× | 0.99× | ✅ |
| tcp-length | 4,707 | 4,763 | 0.9883× | 0.99× | ✅ |
| tcp-eof | 5,202 | 10,231 | 0.5084× | 0.51× | ✅ |
| udp | 7,478 | 7,986 | 0.9363× | 0.93× | ✅ |
| connect | 17,712 | 18,707 | 0.9468× | 0.94× | ✅ |
| https | 108,179 | 108,813 | 0.9942× | 1.0× | ✅ |
| wss | 103,824 | 104,800 | 0.9907× | 0.99× | ✅ |
| atomic | 49,699 | 51,312 | 0.9686× | 0.97× | ✅ |
| process-mode | 15,094 | 16,957 | 0.8902× | 0.88× | ✅ |
| http2-concurrent | 362,180 | 409,267 | 0.8849× | 0.88× | ✅ |
| websocket-64k | 89,944 | 159,696 | 0.5632× | 0.57× | ✅ |
| websocket-fragmented | 11,855 | 16,331 | 0.7259× | 0.73× | ✅ |
| upload | 413,512 | 1,322,387 | 0.3127× | 0.32× | ✅ |
| static | 27,283 | 40,987 | 0.6656× | 0.66× | ✅ |
| pgsql | 23,370 | 24,594 | 0.9502× | 0.95× | ✅ |
| sqlite | 23,545 | 25,236 | 0.9330× | 0.93× | ✅ |
| curl | 86,196 | 90,354 | 0.9540× | 0.94× | ✅ |
| dns | 231,291 | 232,895 | 0.9931× | 0.99× | ✅ |

Geometric mean 0.8572×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
