# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `a42f21e` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,363 | 15,209 | 0.9444× | 0.91× | ✅ |
| echo-1k | 17,349 | 18,169 | 0.9549× | 0.92× | ✅ |
| json | 22,365 | 23,176 | 0.9650× | 0.94× | ✅ |
| service-health | 16,936 | 17,786 | 0.9522× | 0.93× | ✅ |
| websocket | 6,561 | 8,188 | 0.8013× | 0.81× | ✅ |
| timer | 201,531 | 313,591 | 0.6427× | 0.67× | ✅ |
| app | 125,570 | 136,028 | 0.9231× | 0.92× | ✅ |
| http2 | 19,801 | 20,484 | 0.9666× | 0.95× | ✅ |
| table | 197,897 | 201,934 | 0.9800× | 0.98× | ✅ |
| channel | 139,761 | 155,259 | 0.9002× | 0.89× | ✅ |
| task | 31,904 | 33,314 | 0.9577× | 0.94× | ✅ |
| mysql | 49,422 | 51,076 | 0.9676× | 0.97× | ✅ |
| http-client | 33,554 | 35,827 | 0.9366× | 0.95× | ✅ |
| client | 17,927 | 19,624 | 0.9135× | 0.95× | ✅ |
| socket | 18,741 | 18,435 | 1.0166× | 0.99× | ❌ |
| gzip | 200,795 | 197,088 | 1.0188× | 1.02× | ➖ |
| websocket-deflate | 181,365 | 181,568 | 0.9989× | 1.0× | ✅ |
| tcp-length | 5,054 | 4,746 | 1.0649× | 1.02× | ❌ |
| tcp-eof | 5,592 | 10,212 | 0.5476× | 0.55× | ✅ |
| udp | 8,257 | 7,332 | 1.1261× | 1.09× | ❌ |
| https | 109,167 | 108,810 | 1.0033× | 1.04× | ➖ |
| wss | 104,245 | 104,805 | 0.9947× | 1.04× | ✅ |
| atomic | 52,329 | 51,334 | 1.0194× | 1.02× | ➖ |
| process-mode | 16,515 | 16,948 | 0.9744× | 0.95× | ✅ |
| http2-concurrent | 1,469,811 | 408,754 | 3.5958× | 1.31× | ❌ |
| websocket-64k | 156,018 | 159,692 | 0.9770× | 0.62× | ✅ |
| websocket-fragmented | 13,548 | 16,195 | 0.8366× | 0.78× | ✅ |
| upload | 1,780,611 | 1,322,508 | 1.3464× | 1.05× | ❌ |
| static | 39,253 | 41,010 | 0.9571× | 1.03× | ✅ |

Geometric mean 0.9867×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
