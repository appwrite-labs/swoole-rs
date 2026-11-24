# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `e166301` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,766 | 15,214 | 0.9049× | 0.91× | ✅ |
| echo-1k | 16,803 | 18,165 | 0.9250× | 0.92× | ✅ |
| json | 22,095 | 23,170 | 0.9536× | 0.95× | ✅ |
| service-health | 16,352 | 17,786 | 0.9194× | 0.92× | ✅ |
| websocket | 6,726 | 8,189 | 0.8213× | 0.81× | ✅ |
| timer | 202,042 | 313,782 | 0.6439× | 0.65× | ✅ |
| app | 127,551 | 135,919 | 0.9384× | 0.93× | ✅ |
| http2 | 19,008 | 20,500 | 0.9272× | 0.92× | ✅ |
| table | 194,321 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,373 | 155,723 | 0.8822× | 0.88× | ✅ |
| task | 32,087 | 33,268 | 0.9645× | 0.96× | ✅ |
| mysql | 50,293 | 51,088 | 0.9844× | 0.97× | ✅ |
| http-client | 33,604 | 35,796 | 0.9388× | 0.93× | ✅ |
| gzip | 194,301 | 197,099 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,194 | 181,573 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,663 | 4,766 | 0.9783× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,617 | 7,987 | 0.9537× | 0.95× | ✅ |
| connect | 17,538 | 18,734 | 0.9362× | 0.93× | ✅ |
| https | 106,300 | 108,802 | 0.9770× | 0.98× | ✅ |
| wss | 101,582 | 104,806 | 0.9692× | 0.97× | ✅ |
| atomic | 50,207 | 51,285 | 0.9790× | 0.97× | ✅ |
| process-mode | 15,436 | 17,024 | 0.9067× | 0.89× | ✅ |
| http2-concurrent | 363,724 | 409,057 | 0.8892× | 0.89× | ✅ |
| websocket-64k | 89,999 | 159,694 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,928 | 16,372 | 0.7286× | 0.72× | ✅ |
| upload | 416,507 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,850 | 40,999 | 0.6305× | 0.64× | ✅ |
| client | 18,614 | 19,730 | 0.9434× | 0.93× | ✅ |
| socket | 19,857 | 21,046 | 0.9435× | 0.93× | ✅ |
| pgsql | 22,824 | 24,663 | 0.9254× | 0.92× | ✅ |
| sqlite | 23,520 | 25,268 | 0.9308× | 0.93× | ✅ |
| curl | 86,134 | 90,549 | 0.9512× | 0.95× | ✅ |
| dns | 230,163 | 232,800 | 0.9887× | 0.99× | ✅ |
| file | 108,941 | 131,207 | 0.8303× | 0.83× | ✅ |

Geometric mean 0.8536×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
