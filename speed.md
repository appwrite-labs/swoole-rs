# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `ca11421` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,367 | 15,210 | 0.9446× | 0.91× | ✅ |
| echo-1k | 17,356 | 18,172 | 0.9551× | 0.92× | ✅ |
| json | 22,369 | 23,175 | 0.9652× | 0.94× | ✅ |
| service-health | 16,939 | 17,791 | 0.9521× | 0.93× | ✅ |
| websocket | 6,564 | 8,191 | 0.8014× | 0.81× | ✅ |
| timer | 201,395 | 313,416 | 0.6426× | 0.67× | ✅ |
| app | 125,533 | 135,981 | 0.9232× | 0.92× | ✅ |
| http2 | 19,780 | 20,461 | 0.9667× | 0.95× | ✅ |
| table | 196,873 | 201,883 | 0.9752× | 0.98× | ✅ |
| channel | 139,758 | 154,671 | 0.9036× | 0.89× | ✅ |
| task | 31,871 | 33,268 | 0.9580× | 0.94× | ✅ |
| mysql | 49,435 | 51,085 | 0.9677× | 0.97× | ✅ |
| http-client | 33,560 | 35,841 | 0.9364× | 0.95× | ✅ |
| client | 18,132 | 19,381 | 0.9355× | 0.95× | ✅ |
| socket | 19,008 | 20,461 | 0.9290× | 0.99× | ✅ |
| gzip | 200,686 | 197,091 | 1.0182× | 1.02× | ➖ |
| websocket-deflate | 181,365 | 181,576 | 0.9988× | 1.0× | ✅ |
| tcp-length | 5,054 | 4,746 | 1.0649× | 1.02× | ❌ |
| tcp-eof | 5,592 | 10,212 | 0.5476× | 0.55× | ✅ |
| udp | 7,966 | 7,323 | 1.0878× | 1.09× | ➖ |
| connect | 20,303 | 18,712 | 1.0850× | 1.04× | ❌ |
| https | 109,146 | 108,814 | 1.0030× | 1.04× | ➖ |
| wss | 104,242 | 104,794 | 0.9947× | 1.04× | ✅ |
| atomic | 52,329 | 51,334 | 1.0194× | 1.02× | ➖ |
| process-mode | 16,528 | 17,013 | 0.9715× | 0.95× | ✅ |
| http2-concurrent | 1,458,842 | 408,914 | 3.5676× | 1.31× | ❌ |
| websocket-64k | 156,016 | 159,696 | 0.9770× | 0.62× | ✅ |
| websocket-fragmented | 13,549 | 16,194 | 0.8367× | 0.78× | ✅ |
| upload | 1,780,562 | 1,322,423 | 1.3464× | 1.05× | ❌ |
| static | 39,271 | 41,019 | 0.9574× | 1.0× | ✅ |

Geometric mean 0.9862×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
