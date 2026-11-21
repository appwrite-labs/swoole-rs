# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `0ef21d0` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,772 | 15,210 | 0.9054× | 0.91× | ✅ |
| echo-1k | 16,805 | 18,165 | 0.9251× | 0.92× | ✅ |
| json | 22,096 | 23,173 | 0.9535× | 0.95× | ✅ |
| service-health | 16,354 | 17,782 | 0.9197× | 0.92× | ✅ |
| websocket | 6,722 | 8,192 | 0.8205× | 0.81× | ✅ |
| timer | 201,994 | 313,833 | 0.6436× | 0.65× | ✅ |
| app | 127,616 | 135,987 | 0.9384× | 0.93× | ✅ |
| http2 | 18,963 | 20,497 | 0.9252× | 0.92× | ✅ |
| table | 194,323 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,379 | 155,723 | 0.8822× | 0.88× | ✅ |
| task | 32,088 | 33,269 | 0.9645× | 0.96× | ✅ |
| mysql | 50,330 | 51,088 | 0.9852× | 0.97× | ✅ |
| http-client | 33,674 | 35,792 | 0.9408× | 0.93× | ✅ |
| gzip | 194,292 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,570 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,766 | 0.9779× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,986 | 0.9525× | 0.95× | ✅ |
| connect | 17,545 | 18,734 | 0.9366× | 0.93× | ✅ |
| https | 106,323 | 108,802 | 0.9772× | 0.98× | ✅ |
| wss | 101,595 | 104,797 | 0.9694× | 0.97× | ✅ |
| atomic | 50,206 | 51,285 | 0.9790× | 0.97× | ✅ |
| process-mode | 15,426 | 17,024 | 0.9061× | 0.89× | ✅ |
| http2-concurrent | 363,590 | 409,057 | 0.8888× | 0.89× | ✅ |
| websocket-64k | 89,998 | 159,695 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,927 | 16,368 | 0.7287× | 0.72× | ✅ |
| upload | 416,532 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,925 | 41,063 | 0.6313× | 0.64× | ✅ |
| client | 18,631 | 19,730 | 0.9443× | 0.93× | ✅ |
| socket | 19,873 | 21,045 | 0.9443× | 0.93× | ✅ |
| pgsql | 22,840 | 24,662 | 0.9261× | 0.92× | ✅ |
| sqlite | 23,536 | 25,268 | 0.9315× | 0.93× | ✅ |
| curl | 86,146 | 90,419 | 0.9527× | 0.95× | ✅ |
| dns | 230,183 | 232,669 | 0.9893× | 0.99× | ✅ |
| file | 109,104 | 131,297 | 0.8310× | 0.83× | ✅ |

Geometric mean 0.8538×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
