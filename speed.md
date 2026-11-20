# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `9f21772` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,772 | 15,214 | 0.9052× | 0.91× | ✅ |
| echo-1k | 16,802 | 18,165 | 0.9249× | 0.92× | ✅ |
| json | 22,093 | 23,173 | 0.9534× | 0.95× | ✅ |
| service-health | 16,351 | 17,786 | 0.9193× | 0.92× | ✅ |
| websocket | 6,722 | 8,189 | 0.8209× | 0.81× | ✅ |
| timer | 201,944 | 313,778 | 0.6436× | 0.65× | ✅ |
| app | 127,638 | 135,950 | 0.9389× | 0.93× | ✅ |
| http2 | 18,969 | 20,500 | 0.9253× | 0.92× | ✅ |
| table | 194,323 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,375 | 155,726 | 0.8822× | 0.88× | ✅ |
| task | 32,085 | 33,268 | 0.9644× | 0.96× | ✅ |
| mysql | 50,325 | 51,088 | 0.9850× | 0.97× | ✅ |
| http-client | 33,671 | 35,796 | 0.9406× | 0.93× | ✅ |
| gzip | 194,296 | 197,099 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,573 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,766 | 0.9779× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,986 | 0.9526× | 0.95× | ✅ |
| connect | 17,537 | 18,734 | 0.9361× | 0.93× | ✅ |
| https | 106,323 | 108,802 | 0.9772× | 0.98× | ✅ |
| wss | 101,595 | 104,797 | 0.9694× | 0.97× | ✅ |
| atomic | 50,206 | 51,285 | 0.9790× | 0.97× | ✅ |
| process-mode | 15,425 | 17,024 | 0.9061× | 0.89× | ✅ |
| http2-concurrent | 363,612 | 409,057 | 0.8889× | 0.89× | ✅ |
| websocket-64k | 90,002 | 159,695 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,372 | 0.7282× | 0.72× | ✅ |
| upload | 416,526 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,861 | 41,028 | 0.6303× | 0.64× | ✅ |
| client | 18,636 | 19,729 | 0.9446× | 0.93× | ✅ |
| socket | 19,873 | 21,046 | 0.9443× | 0.93× | ✅ |
| pgsql | 22,831 | 24,662 | 0.9258× | 0.92× | ✅ |
| sqlite | 23,532 | 25,268 | 0.9313× | 0.93× | ✅ |
| curl | 86,146 | 90,422 | 0.9527× | 0.95× | ✅ |
| dns | 229,937 | 232,676 | 0.9882× | 0.99× | ✅ |
| file | 109,097 | 131,300 | 0.8309× | 0.83× | ✅ |

Geometric mean 0.8536×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
