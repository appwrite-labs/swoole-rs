# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `62befbf` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 24,779 | 15,213 | 1.6288× | 0.95× | ❌ |
| echo-1k | 32,671 | 18,170 | 1.7981× | 0.96× | ❌ |
| json | 33,349 | 23,178 | 1.4388× | 0.97× | ❌ |
| service-health | 27,556 | 17,795 | 1.5485× | 0.96× | ❌ |
| websocket | 6,593 | 8,188 | 0.8052× | 0.81× | ✅ |
| timer | 211,754 | 313,405 | 0.6757× | 0.65× | ✅ |
| app | 140,866 | 135,900 | 1.0365× | 0.92× | ❌ |
| http2 | 19,628 | 20,464 | 0.9591× | 0.97× | ✅ |
| table | 207,970 | 202,907 | 1.0250× | 0.98× | ❌ |
| channel | 148,147 | 155,189 | 0.9546× | 0.89× | ✅ |
| task | 47,035 | 33,265 | 1.4140× | 0.96× | ❌ |
| mysql | 59,969 | 51,073 | 1.1742× | 0.97× | ❌ |
| http-client | 52,591 | 35,825 | 1.4680× | 0.94× | ❌ |
| client | 29,232 | 19,775 | 1.4782× | 0.95× | ❌ |
| socket | 30,707 | 21,087 | 1.4562× | 0.99× | ❌ |
| gzip | 215,860 | 197,095 | 1.0952× | 1.02× | ❌ |
| websocket-deflate | 181,391 | 181,564 | 0.9990× | 1.0× | ✅ |
| tcp-length | 5,066 | 4,746 | 1.0675× | 1.07× | ➖ |
| tcp-eof | 5,604 | 10,209 | 0.5490× | 0.55× | ✅ |
| udp | 7,512 | 7,516 | 0.9994× | 1.07× | ✅ |
| connect | 33,393 | 18,707 | 1.7851× | 1.09× | ❌ |
| https | 119,556 | 108,810 | 1.0988× | 1.01× | ❌ |
| wss | 104,277 | 104,803 | 0.9950× | 1.0× | ✅ |
| atomic | 66,166 | 51,334 | 1.2889× | 1.02× | ❌ |
| process-mode | 26,912 | 16,946 | 1.5882× | 0.98× | ❌ |
| http2-concurrent | 1,457,166 | 408,818 | 3.5643× | 1.31× | ❌ |
| websocket-64k | 156,046 | 159,694 | 0.9772× | 0.62× | ✅ |
| websocket-fragmented | 13,931 | 16,195 | 0.8602× | 0.78× | ✅ |
| upload | 1,799,206 | 1,322,459 | 1.3605× | 1.05× | ❌ |
| static | 39,245 | 40,999 | 0.9572× | 0.96× | ✅ |

Geometric mean 1.1892×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
