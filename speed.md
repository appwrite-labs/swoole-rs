# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `6798c5c` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,790 | 15,214 | 0.9722× | 0.95× | ✅ |
| echo-1k | 17,997 | 18,169 | 0.9905× | 0.96× | ✅ |
| json | 22,845 | 23,178 | 0.9857× | 0.97× | ✅ |
| service-health | 17,398 | 17,795 | 0.9777× | 0.96× | ✅ |
| websocket | 6,944 | 8,188 | 0.8481× | 0.81× | ✅ |
| timer | 201,764 | 313,475 | 0.6436× | 0.65× | ✅ |
| app | 127,691 | 135,849 | 0.9399× | 0.92× | ✅ |
| http2 | 20,451 | 20,467 | 0.9992× | 0.97× | ✅ |
| table | 197,774 | 202,907 | 0.9747× | 0.98× | ✅ |
| channel | 140,409 | 155,192 | 0.9047× | 0.89× | ✅ |
| task | 32,864 | 33,265 | 0.9879× | 0.96× | ✅ |
| mysql | 50,380 | 51,065 | 0.9866× | 0.97× | ✅ |
| http-client | 34,926 | 35,827 | 0.9748× | 0.94× | ✅ |
| client | 19,338 | 19,775 | 0.9779× | 0.95× | ✅ |
| socket | 20,836 | 21,091 | 0.9879× | 0.99× | ✅ |
| gzip | 197,164 | 197,092 | 1.0004× | 1.02× | ➖ |
| websocket-deflate | 181,743 | 181,564 | 1.0010× | 1.0× | ❌ |
| tcp-length | 5,467 | 4,743 | 1.1526× | 1.07× | ❌ |
| tcp-eof | 5,955 | 10,209 | 0.5833× | 0.55× | ✅ |
| udp | 7,226 | 7,349 | 0.9833× | 1.04× | ✅ |
| connect | 21,691 | 18,707 | 1.1595× | 1.09× | ❌ |
| https | 110,210 | 108,810 | 1.0129× | 1.01× | ❌ |
| wss | 105,383 | 104,803 | 1.0055× | 1.0× | ❌ |
| atomic | 53,253 | 51,334 | 1.0374× | 1.06× | ➖ |
| process-mode | 17,217 | 16,946 | 1.0160× | 0.98× | ❌ |
| http2-concurrent | 443,433 | 408,813 | 1.0847× | 0.93× | ❌ |
| websocket-64k | 156,395 | 159,692 | 0.9794× | 0.62× | ✅ |
| websocket-fragmented | 14,171 | 16,194 | 0.8751× | 0.78× | ✅ |
| upload | 674,855 | 1,322,423 | 0.5103× | 1.05× | ✅ |
| static | 40,874 | 41,028 | 0.9962× | 0.96× | ✅ |

Geometric mean 0.9387×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
