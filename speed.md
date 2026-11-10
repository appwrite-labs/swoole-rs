# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `b238d7e` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,436 | 15,210 | 0.9491× | 0.95× | ✅ |
| echo-1k | 17,601 | 18,166 | 0.9689× | 0.96× | ✅ |
| json | 22,493 | 23,178 | 0.9705× | 0.97× | ✅ |
| service-health | 17,034 | 17,791 | 0.9575× | 0.96× | ✅ |
| websocket | 6,586 | 8,191 | 0.8040× | 0.81× | ✅ |
| timer | 201,355 | 313,362 | 0.6426× | 0.65× | ✅ |
| app | 125,360 | 135,964 | 0.9220× | 0.92× | ✅ |
| http2 | 19,654 | 20,467 | 0.9603× | 0.97× | ✅ |
| table | 197,480 | 202,907 | 0.9733× | 0.98× | ✅ |
| channel | 137,578 | 155,192 | 0.8865× | 0.89× | ✅ |
| task | 32,074 | 33,277 | 0.9638× | 0.96× | ✅ |
| mysql | 49,504 | 51,079 | 0.9691× | 0.97× | ✅ |
| http-client | 33,947 | 35,821 | 0.9477× | 0.94× | ✅ |
| client | 18,705 | 19,774 | 0.9459× | 0.95× | ✅ |
| socket | 20,184 | 21,091 | 0.9570× | 0.99× | ✅ |
| gzip | 200,933 | 197,094 | 1.0195× | 1.02× | ➖ |
| websocket-deflate | 181,381 | 181,560 | 0.9990× | 1.0× | ✅ |
| tcp-length | 5,074 | 4,746 | 1.0691× | 1.07× | ➖ |
| tcp-eof | 5,585 | 10,212 | 0.5469× | 0.55× | ✅ |
| udp | 7,152 | 7,328 | 0.9760× | 1.07× | ✅ |
| connect | 20,494 | 18,707 | 1.0955× | 1.09× | ❌ |
| https | 109,222 | 108,810 | 1.0038× | 1.01× | ➖ |
| wss | 104,290 | 104,794 | 0.9952× | 1.0× | ✅ |
| atomic | 54,040 | 51,334 | 1.0527× | 1.06× | ➖ |
| process-mode | 16,565 | 16,949 | 0.9774× | 0.98× | ✅ |
| http2-concurrent | 438,416 | 408,816 | 1.0724× | 0.93× | ❌ |
| websocket-64k | 156,040 | 159,694 | 0.9771× | 0.62× | ✅ |
| websocket-fragmented | 13,507 | 16,194 | 0.8341× | 0.78× | ✅ |
| upload | 674,499 | 1,322,480 | 0.5100× | 1.05× | ✅ |
| static | 39,262 | 41,028 | 0.9569× | 0.96× | ✅ |

Geometric mean 0.9175×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
