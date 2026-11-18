# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `098ca9e` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,737 | 15,210 | 0.9031× | 0.91× | ✅ |
| echo-1k | 16,728 | 18,162 | 0.9210× | 0.92× | ✅ |
| json | 22,069 | 23,164 | 0.9527× | 0.95× | ✅ |
| service-health | 16,308 | 17,773 | 0.9176× | 0.92× | ✅ |
| websocket | 6,679 | 8,192 | 0.8153× | 0.8× | ✅ |
| timer | 201,355 | 314,198 | 0.6409× | 0.65× | ✅ |
| app | 126,696 | 135,936 | 0.9320× | 0.93× | ✅ |
| http2 | 18,970 | 20,497 | 0.9255× | 0.92× | ✅ |
| table | 193,449 | 202,900 | 0.9534× | 0.96× | ✅ |
| channel | 137,324 | 155,714 | 0.8819× | 0.88× | ✅ |
| task | 31,932 | 33,272 | 0.9597× | 0.96× | ✅ |
| mysql | 49,772 | 51,064 | 0.9747× | 0.97× | ✅ |
| http-client | 33,572 | 35,791 | 0.9380× | 0.93× | ✅ |
| gzip | 194,334 | 197,095 | 0.9860× | 0.99× | ✅ |
| websocket-deflate | 179,110 | 181,570 | 0.9865× | 0.99× | ✅ |
| tcp-length | 4,643 | 4,763 | 0.9748× | 0.96× | ✅ |
| tcp-eof | 5,193 | 10,228 | 0.5077× | 0.5× | ✅ |
| udp | 7,576 | 7,986 | 0.9486× | 0.95× | ✅ |
| connect | 17,572 | 18,734 | 0.9380× | 0.93× | ✅ |
| https | 106,025 | 108,802 | 0.9745× | 0.98× | ✅ |
| wss | 101,242 | 104,797 | 0.9661× | 0.97× | ✅ |
| atomic | 50,167 | 51,285 | 0.9782× | 0.97× | ✅ |
| process-mode | 15,255 | 17,024 | 0.8961× | 0.89× | ✅ |
| http2-concurrent | 366,031 | 409,057 | 0.8948× | 0.89× | ✅ |
| websocket-64k | 89,977 | 159,694 | 0.5634× | 0.57× | ✅ |
| websocket-fragmented | 11,845 | 16,372 | 0.7235× | 0.72× | ✅ |
| upload | 416,351 | 1,322,432 | 0.3148× | 0.32× | ✅ |
| static | 27,144 | 41,092 | 0.6606× | 0.65× | ✅ |
| client | 18,356 | 19,730 | 0.9303× | 0.93× | ✅ |
| socket | 19,575 | 21,045 | 0.9301× | 0.93× | ✅ |
| pgsql | 22,585 | 24,618 | 0.9174× | 0.92× | ✅ |
| sqlite | 23,399 | 25,236 | 0.9272× | 0.93× | ✅ |
| curl | 85,342 | 90,360 | 0.9445× | 0.95× | ✅ |
| dns | 229,684 | 232,444 | 0.9881× | 0.99× | ✅ |

Geometric mean 0.8528×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
