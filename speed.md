# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `fd3d458` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,436 | 15,213 | 0.9489× | 0.95× | ✅ |
| echo-1k | 17,607 | 18,163 | 0.9694× | 0.96× | ✅ |
| json | 22,493 | 23,174 | 0.9706× | 0.97× | ✅ |
| service-health | 17,034 | 17,791 | 0.9575× | 0.96× | ✅ |
| websocket | 6,590 | 8,191 | 0.8045× | 0.81× | ✅ |
| timer | 201,372 | 313,359 | 0.6426× | 0.65× | ✅ |
| app | 125,379 | 135,977 | 0.9221× | 0.92× | ✅ |
| http2 | 19,657 | 20,463 | 0.9606× | 0.97× | ✅ |
| table | 197,484 | 202,907 | 0.9733× | 0.98× | ✅ |
| channel | 137,582 | 155,193 | 0.8865× | 0.89× | ✅ |
| task | 32,075 | 33,269 | 0.9641× | 0.96× | ✅ |
| mysql | 49,500 | 51,047 | 0.9697× | 0.97× | ✅ |
| http-client | 33,951 | 35,823 | 0.9477× | 0.94× | ✅ |
| client | 18,706 | 19,775 | 0.9459× | 0.95× | ✅ |
| socket | 20,185 | 21,091 | 0.9570× | 0.99× | ✅ |
| gzip | 196,777 | 197,094 | 0.9984× | 1.02× | ✅ |
| websocket-deflate | 181,381 | 181,564 | 0.9990× | 1.0× | ✅ |
| tcp-length | 5,074 | 4,743 | 1.0699× | 1.07× | ➖ |
| tcp-eof | 5,585 | 10,212 | 0.5469× | 0.55× | ✅ |
| udp | 7,163 | 7,334 | 0.9768× | 1.04× | ✅ |
| connect | 20,494 | 18,707 | 1.0955× | 1.09× | ❌ |
| https | 109,206 | 108,803 | 1.0037× | 1.01× | ➖ |
| wss | 104,282 | 104,797 | 0.9951× | 1.0× | ✅ |
| atomic | 52,888 | 51,334 | 1.0303× | 1.06× | ➖ |
| process-mode | 16,563 | 16,948 | 0.9773× | 0.98× | ✅ |
| http2-concurrent | 438,713 | 408,816 | 1.0731× | 0.93× | ❌ |
| websocket-64k | 156,034 | 159,697 | 0.9771× | 0.62× | ✅ |
| websocket-fragmented | 13,508 | 16,194 | 0.8341× | 0.78× | ✅ |
| upload | 674,489 | 1,322,477 | 0.5100× | 1.05× | ✅ |
| static | 39,233 | 40,999 | 0.9569× | 0.96× | ✅ |

Geometric mean 0.9163×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
