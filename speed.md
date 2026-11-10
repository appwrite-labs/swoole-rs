# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `52b53db` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 24,913 | 15,214 | 1.6376× | 0.95× | ❌ |
| echo-1k | 32,660 | 18,175 | 1.7970× | 0.96× | ❌ |
| json | 33,480 | 23,178 | 1.4445× | 0.97× | ❌ |
| service-health | 27,705 | 17,791 | 1.5572× | 0.96× | ❌ |
| websocket | 6,606 | 8,191 | 0.8065× | 0.81× | ✅ |
| timer | 211,840 | 313,376 | 0.6760× | 0.65× | ✅ |
| app | 140,807 | 135,948 | 1.0357× | 0.92× | ❌ |
| http2 | 19,665 | 20,464 | 0.9609× | 0.97× | ✅ |
| table | 208,115 | 202,907 | 1.0257× | 0.98× | ❌ |
| channel | 148,288 | 155,192 | 0.9555× | 0.89× | ✅ |
| task | 46,966 | 33,263 | 1.4120× | 0.96× | ❌ |
| mysql | 60,097 | 51,076 | 1.1766× | 0.97× | ❌ |
| http-client | 52,565 | 35,824 | 1.4673× | 0.94× | ❌ |
| client | 29,376 | 19,775 | 1.4855× | 0.95× | ❌ |
| socket | 30,854 | 21,090 | 1.4630× | 0.99× | ❌ |
| gzip | 215,841 | 197,089 | 1.0951× | 1.02× | ❌ |
| websocket-deflate | 181,407 | 181,564 | 0.9991× | 1.0× | ✅ |
| tcp-length | 5,074 | 4,746 | 1.0691× | 1.07× | ➖ |
| tcp-eof | 5,585 | 10,212 | 0.5469× | 0.55× | ✅ |
| udp | 7,575 | 7,322 | 1.0346× | 1.07× | ➖ |
| connect | 33,480 | 18,707 | 1.7897× | 1.09× | ❌ |
| https | 119,687 | 108,810 | 1.1000× | 1.01× | ❌ |
| wss | 104,295 | 104,794 | 0.9952× | 1.0× | ✅ |
| atomic | 66,311 | 51,334 | 1.2918× | 1.02× | ❌ |
| process-mode | 27,035 | 16,948 | 1.5952× | 0.98× | ❌ |
| http2-concurrent | 438,729 | 408,813 | 1.0732× | 0.93× | ❌ |
| websocket-64k | 156,056 | 159,693 | 0.9772× | 0.62× | ✅ |
| websocket-fragmented | 13,527 | 16,194 | 0.8353× | 0.78× | ✅ |
| upload | 1,799,909 | 1,322,451 | 1.3610× | 1.05× | ❌ |
| static | 39,262 | 41,028 | 0.9569× | 0.96× | ✅ |

Geometric mean 1.1442×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
