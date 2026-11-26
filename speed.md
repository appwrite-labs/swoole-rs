# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `a228706` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,742 | 15,213 | 0.9033× | 0.91× | ✅ |
| echo-1k | 16,781 | 18,165 | 0.9238× | 0.92× | ✅ |
| json | 22,078 | 23,173 | 0.9527× | 0.95× | ✅ |
| service-health | 16,332 | 17,786 | 0.9182× | 0.92× | ✅ |
| websocket | 6,723 | 8,189 | 0.8211× | 0.81× | ✅ |
| timer | 202,211 | 313,770 | 0.6445× | 0.65× | ✅ |
| app | 127,588 | 135,950 | 0.9385× | 0.93× | ✅ |
| http2 | 19,188 | 20,501 | 0.9359× | 0.92× | ✅ |
| table | 194,308 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,046 | 155,726 | 0.8800× | 0.88× | ✅ |
| task | 32,109 | 33,272 | 0.9651× | 0.96× | ✅ |
| mysql | 50,323 | 51,085 | 0.9851× | 0.97× | ✅ |
| http-client | 33,681 | 35,797 | 0.9409× | 0.93× | ✅ |
| gzip | 194,272 | 197,095 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,987 | 181,570 | 0.9913× | 0.99× | ✅ |
| tcp-length | 4,662 | 4,763 | 0.9788× | 0.96× | ✅ |
| tcp-eof | 5,067 | 10,231 | 0.4952× | 0.5× | ✅ |
| udp | 7,616 | 7,986 | 0.9537× | 0.95× | ✅ |
| connect | 17,516 | 18,734 | 0.9350× | 0.93× | ✅ |
| https | 106,276 | 108,809 | 0.9767× | 0.98× | ✅ |
| wss | 101,560 | 104,797 | 0.9691× | 0.97× | ✅ |
| atomic | 50,452 | 51,276 | 0.9839× | 0.97× | ✅ |
| process-mode | 15,430 | 17,021 | 0.9065× | 0.89× | ✅ |
| http2-concurrent | 367,767 | 408,865 | 0.8995× | 0.89× | ✅ |
| websocket-64k | 90,004 | 159,695 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,925 | 16,372 | 0.7284× | 0.72× | ✅ |
| upload | 418,888 | 1,322,425 | 0.3168× | 0.32× | ✅ |
| static | 25,294 | 41,063 | 0.6160× | 0.62× | ✅ |
| client | 18,623 | 19,720 | 0.9443× | 0.93× | ✅ |
| socket | 19,865 | 21,036 | 0.9443× | 0.93× | ✅ |
| pgsql | 22,921 | 24,663 | 0.9294× | 0.92× | ✅ |
| sqlite | 23,511 | 25,268 | 0.9305× | 0.93× | ✅ |
| curl | 86,114 | 90,545 | 0.9511× | 0.95× | ✅ |
| dns | 230,350 | 232,752 | 0.9897× | 0.99× | ✅ |
| file | 108,909 | 131,300 | 0.8295× | 0.83× | ✅ |

Geometric mean 0.8539×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
