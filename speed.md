# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `82d0fe4` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,946 | 15,210 | 0.9169× | 0.92× | ✅ |
| echo-1k | 17,050 | 18,165 | 0.9386× | 0.94× | ✅ |
| json | 22,113 | 23,161 | 0.9547× | 0.95× | ✅ |
| service-health | 16,550 | 17,773 | 0.9312× | 0.93× | ✅ |
| websocket | 6,596 | 8,191 | 0.8052× | 0.8× | ✅ |
| timer | 206,129 | 313,592 | 0.6573× | 0.65× | ✅ |
| app | 125,436 | 135,946 | 0.9227× | 0.93× | ✅ |
| http2 | 18,898 | 20,465 | 0.9235× | 0.91× | ✅ |
| table | 195,113 | 201,865 | 0.9666× | 0.97× | ✅ |
| channel | 137,257 | 155,167 | 0.8846× | 0.88× | ✅ |
| task | 32,538 | 33,261 | 0.9783× | 0.96× | ✅ |
| mysql | 49,569 | 51,049 | 0.9710× | 0.97× | ✅ |
| http-client | 33,915 | 35,801 | 0.9473× | 0.94× | ✅ |
| client | 18,546 | 19,753 | 0.9389× | 0.94× | ✅ |
| socket | 19,697 | 21,073 | 0.9347× | 0.92× | ✅ |
| gzip | 194,153 | 197,079 | 0.9852× | 0.99× | ✅ |
| websocket-deflate | 179,034 | 181,560 | 0.9861× | 0.99× | ✅ |
| tcp-length | 4,679 | 4,762 | 0.9826× | 0.99× | ✅ |
| tcp-eof | 5,174 | 10,231 | 0.5057× | 0.51× | ✅ |
| udp | 7,460 | 7,987 | 0.9340× | 0.93× | ✅ |
| connect | 17,680 | 18,703 | 0.9453× | 0.94× | ✅ |
| https | 108,174 | 108,813 | 0.9941× | 1.0× | ✅ |
| wss | 103,794 | 104,800 | 0.9904× | 0.99× | ✅ |
| atomic | 49,772 | 51,316 | 0.9699× | 0.97× | ✅ |
| process-mode | 15,064 | 16,960 | 0.8882× | 0.88× | ✅ |
| http2-concurrent | 366,442 | 409,074 | 0.8958× | 0.88× | ✅ |
| websocket-64k | 89,926 | 159,696 | 0.5631× | 0.57× | ✅ |
| websocket-fragmented | 11,849 | 16,327 | 0.7257× | 0.73× | ✅ |
| upload | 413,526 | 1,322,389 | 0.3127× | 0.32× | ✅ |
| static | 27,347 | 41,016 | 0.6667× | 0.66× | ✅ |
| pgsql | 23,343 | 24,594 | 0.9491× | 0.95× | ✅ |
| sqlite | 23,559 | 25,236 | 0.9335× | 0.93× | ✅ |
| curl | 86,114 | 90,352 | 0.9531× | 0.94× | ✅ |
| dns | 232,007 | 233,177 | 0.9950× | 0.99× | ✅ |

Geometric mean 0.8576×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
