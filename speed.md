# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `616b80b` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,372 | 15,210 | 0.9449× | 0.91× | ✅ |
| echo-1k | 17,363 | 18,174 | 0.9553× | 0.92× | ✅ |
| json | 22,369 | 23,174 | 0.9652× | 0.94× | ✅ |
| service-health | 16,939 | 17,791 | 0.9521× | 0.93× | ✅ |
| websocket | 6,561 | 8,191 | 0.8010× | 0.81× | ✅ |
| timer | 201,416 | 313,371 | 0.6427× | 0.67× | ✅ |
| app | 125,272 | 135,854 | 0.9221× | 0.92× | ✅ |
| http2 | 19,782 | 20,464 | 0.9667× | 0.95× | ✅ |
| table | 197,385 | 202,907 | 0.9728× | 0.98× | ✅ |
| channel | 139,240 | 155,189 | 0.8972× | 0.89× | ✅ |
| task | 31,868 | 33,270 | 0.9579× | 0.94× | ✅ |
| mysql | 49,421 | 51,076 | 0.9676× | 0.97× | ✅ |
| http-client | 33,550 | 35,823 | 0.9365× | 0.95× | ✅ |
| client | 18,436 | 19,454 | 0.9476× | 0.95× | ✅ |
| socket | 19,555 | 18,577 | 1.0527× | 0.99× | ❌ |
| gzip | 200,679 | 197,095 | 1.0182× | 1.02× | ➖ |
| websocket-deflate | 181,365 | 181,564 | 0.9989× | 1.0× | ✅ |
| tcp-length | 5,054 | 4,746 | 1.0649× | 1.02× | ❌ |
| tcp-eof | 5,592 | 10,212 | 0.5476× | 0.55× | ✅ |
| udp | 8,226 | 7,363 | 1.1171× | 1.09× | ❌ |
| connect | 20,321 | 18,707 | 1.0863× | 1.04× | ❌ |
| https | 109,153 | 108,810 | 1.0031× | 1.04× | ➖ |
| wss | 104,259 | 104,794 | 0.9949× | 1.04× | ✅ |
| atomic | 52,329 | 51,334 | 1.0194× | 1.02× | ➖ |
| process-mode | 16,526 | 17,010 | 0.9716× | 0.95× | ✅ |
| http2-concurrent | 1,459,126 | 408,813 | 3.5692× | 1.31× | ❌ |
| websocket-64k | 156,015 | 159,691 | 0.9770× | 0.62× | ✅ |
| websocket-fragmented | 13,549 | 16,194 | 0.8367× | 0.78× | ✅ |
| upload | 1,780,615 | 1,322,462 | 1.3464× | 1.05× | ❌ |
| static | 39,227 | 40,999 | 0.9568× | 1.0× | ✅ |

Geometric mean 0.9913×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
