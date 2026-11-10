# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `0f29acb` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,381 | 15,210 | 0.9455× | 0.91× | ✅ |
| echo-1k | 17,367 | 18,159 | 0.9564× | 0.92× | ✅ |
| json | 22,409 | 23,175 | 0.9670× | 0.94× | ✅ |
| service-health | 16,980 | 17,791 | 0.9544× | 0.93× | ✅ |
| websocket | 6,565 | 8,191 | 0.8014× | 0.81× | ✅ |
| timer | 201,569 | 313,314 | 0.6433× | 0.67× | ✅ |
| app | 125,550 | 135,873 | 0.9240× | 0.92× | ✅ |
| http2 | 19,792 | 20,464 | 0.9672× | 0.95× | ✅ |
| table | 196,914 | 201,883 | 0.9754× | 0.98× | ✅ |
| channel | 139,799 | 154,671 | 0.9038× | 0.89× | ✅ |
| task | 31,885 | 33,273 | 0.9583× | 0.94× | ✅ |
| mysql | 49,376 | 51,073 | 0.9668× | 0.97× | ✅ |
| http-client | 33,581 | 35,840 | 0.9370× | 0.95× | ✅ |
| client | 18,552 | 19,565 | 0.9482× | 0.95× | ✅ |
| socket | 19,113 | 18,171 | 1.0518× | 0.99× | ❌ |
| gzip | 200,799 | 197,093 | 1.0188× | 1.02× | ➖ |
| websocket-deflate | 181,365 | 181,576 | 0.9988× | 1.0× | ✅ |
| tcp-length | 5,054 | 4,746 | 1.0649× | 1.02× | ❌ |
| tcp-eof | 5,592 | 10,212 | 0.5476× | 0.55× | ✅ |
| udp | 7,881 | 7,322 | 1.0763× | 1.09× | ➖ |
| connect | 20,303 | 18,712 | 1.0850× | 1.04× | ❌ |
| https | 109,163 | 108,810 | 1.0032× | 1.04× | ➖ |
| wss | 104,259 | 104,797 | 0.9949× | 1.04× | ✅ |
| atomic | 52,370 | 51,334 | 1.0202× | 1.02× | ❌ |
| process-mode | 16,515 | 16,945 | 0.9746× | 0.95× | ✅ |
| http2-concurrent | 1,469,667 | 408,909 | 3.5941× | 1.31× | ❌ |
| websocket-64k | 156,013 | 159,693 | 0.9770× | 0.62× | ✅ |
| websocket-fragmented | 13,549 | 16,194 | 0.8367× | 0.78× | ✅ |
| upload | 1,780,599 | 1,322,457 | 1.3464× | 1.05× | ❌ |
| static | 39,209 | 40,981 | 0.9568× | 1.03× | ✅ |

Geometric mean 0.9911×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
