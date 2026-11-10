# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `dbef69e` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,433 | 15,214 | 0.9487× | 0.95× | ✅ |
| echo-1k | 17,601 | 18,162 | 0.9691× | 0.96× | ✅ |
| json | 22,493 | 23,174 | 0.9706× | 0.97× | ✅ |
| service-health | 17,034 | 17,791 | 0.9575× | 0.96× | ✅ |
| websocket | 6,590 | 8,191 | 0.8045× | 0.81× | ✅ |
| timer | 201,334 | 313,331 | 0.6426× | 0.65× | ✅ |
| app | 125,398 | 135,978 | 0.9222× | 0.92× | ✅ |
| http2 | 19,656 | 20,463 | 0.9605× | 0.97× | ✅ |
| table | 197,484 | 202,911 | 0.9733× | 0.98× | ✅ |
| channel | 137,582 | 155,189 | 0.8865× | 0.89× | ✅ |
| task | 32,077 | 33,266 | 0.9642× | 0.96× | ✅ |
| mysql | 49,484 | 51,076 | 0.9688× | 0.97× | ✅ |
| http-client | 33,956 | 35,824 | 0.9479× | 0.94× | ✅ |
| client | 18,705 | 19,775 | 0.9459× | 0.95× | ✅ |
| socket | 20,185 | 21,091 | 0.9570× | 0.99× | ✅ |
| gzip | 196,776 | 197,093 | 0.9984× | 1.02× | ✅ |
| websocket-deflate | 181,384 | 181,564 | 0.9990× | 1.0× | ✅ |
| tcp-length | 5,074 | 4,743 | 1.0699× | 1.07× | ➖ |
| tcp-eof | 5,585 | 10,212 | 0.5469× | 0.55× | ✅ |
| udp | 7,838 | 7,637 | 1.0264× | 1.04× | ➖ |
| connect | 20,490 | 18,707 | 1.0953× | 1.09× | ❌ |
| https | 109,206 | 108,810 | 1.0036× | 1.01× | ➖ |
| wss | 104,282 | 104,794 | 0.9951× | 1.0× | ✅ |
| atomic | 52,888 | 51,334 | 1.0303× | 1.06× | ➖ |
| process-mode | 16,566 | 16,945 | 0.9776× | 0.98× | ✅ |
| http2-concurrent | 438,625 | 408,818 | 1.0729× | 0.93× | ❌ |
| websocket-64k | 156,039 | 159,695 | 0.9771× | 0.62× | ✅ |
| websocket-fragmented | 13,507 | 16,194 | 0.8341× | 0.78× | ✅ |
| upload | 674,545 | 1,322,469 | 0.5101× | 1.05× | ✅ |
| static | 39,262 | 41,028 | 0.9569× | 0.96× | ✅ |

Geometric mean 0.9178×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
