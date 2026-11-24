# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `e166301` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,771 | 15,213 | 0.9052× | 0.91× | ✅ |
| echo-1k | 16,799 | 18,165 | 0.9248× | 0.92× | ✅ |
| json | 22,091 | 23,173 | 0.9533× | 0.95× | ✅ |
| service-health | 16,352 | 17,786 | 0.9194× | 0.92× | ✅ |
| websocket | 6,722 | 8,192 | 0.8205× | 0.81× | ✅ |
| timer | 201,991 | 313,747 | 0.6438× | 0.65× | ✅ |
| app | 127,493 | 135,940 | 0.9379× | 0.93× | ✅ |
| http2 | 19,003 | 20,496 | 0.9272× | 0.92× | ✅ |
| table | 194,321 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,377 | 155,726 | 0.8822× | 0.88× | ✅ |
| task | 32,088 | 33,265 | 0.9646× | 0.96× | ✅ |
| mysql | 50,297 | 51,085 | 0.9846× | 0.97× | ✅ |
| http-client | 33,599 | 35,797 | 0.9386× | 0.93× | ✅ |
| gzip | 194,301 | 197,099 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,194 | 181,573 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,663 | 4,766 | 0.9783× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,617 | 7,986 | 0.9538× | 0.95× | ✅ |
| connect | 17,541 | 18,730 | 0.9365× | 0.93× | ✅ |
| https | 106,300 | 108,809 | 0.9769× | 0.98× | ✅ |
| wss | 101,573 | 104,804 | 0.9692× | 0.97× | ✅ |
| atomic | 50,207 | 51,285 | 0.9790× | 0.97× | ✅ |
| process-mode | 15,436 | 17,024 | 0.9067× | 0.89× | ✅ |
| http2-concurrent | 363,874 | 409,058 | 0.8895× | 0.89× | ✅ |
| websocket-64k | 90,008 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,924 | 16,372 | 0.7283× | 0.72× | ✅ |
| upload | 416,508 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,856 | 41,005 | 0.6305× | 0.64× | ✅ |
| client | 18,614 | 19,726 | 0.9436× | 0.93× | ✅ |
| socket | 19,854 | 21,046 | 0.9434× | 0.93× | ✅ |
| pgsql | 22,822 | 24,663 | 0.9254× | 0.92× | ✅ |
| sqlite | 23,520 | 25,268 | 0.9308× | 0.93× | ✅ |
| curl | 86,140 | 90,421 | 0.9526× | 0.95× | ✅ |
| dns | 229,972 | 232,582 | 0.9888× | 0.99× | ✅ |
| file | 108,937 | 131,300 | 0.8297× | 0.83× | ✅ |

Geometric mean 0.8536×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
