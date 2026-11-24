# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `e166301` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,771 | 15,214 | 0.9051× | 0.91× | ✅ |
| echo-1k | 16,799 | 18,166 | 0.9248× | 0.92× | ✅ |
| json | 22,091 | 23,173 | 0.9533× | 0.95× | ✅ |
| service-health | 16,349 | 17,786 | 0.9192× | 0.92× | ✅ |
| websocket | 6,721 | 8,189 | 0.8208× | 0.81× | ✅ |
| timer | 202,021 | 313,906 | 0.6436× | 0.65× | ✅ |
| app | 127,541 | 135,884 | 0.9386× | 0.93× | ✅ |
| http2 | 19,009 | 20,500 | 0.9272× | 0.92× | ✅ |
| table | 194,321 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,373 | 155,726 | 0.8821× | 0.88× | ✅ |
| task | 32,091 | 33,268 | 0.9646× | 0.96× | ✅ |
| mysql | 50,293 | 51,085 | 0.9845× | 0.97× | ✅ |
| http-client | 33,604 | 35,796 | 0.9388× | 0.93× | ✅ |
| gzip | 194,297 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,191 | 181,570 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,663 | 4,766 | 0.9783× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,617 | 7,987 | 0.9538× | 0.95× | ✅ |
| connect | 17,538 | 18,734 | 0.9362× | 0.93× | ✅ |
| https | 106,300 | 108,809 | 0.9769× | 0.98× | ✅ |
| wss | 101,582 | 104,797 | 0.9693× | 0.97× | ✅ |
| atomic | 50,207 | 51,285 | 0.9790× | 0.97× | ✅ |
| process-mode | 15,436 | 17,024 | 0.9067× | 0.89× | ✅ |
| http2-concurrent | 364,034 | 409,057 | 0.8899× | 0.89× | ✅ |
| websocket-64k | 89,999 | 159,694 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,927 | 16,368 | 0.7287× | 0.72× | ✅ |
| upload | 416,511 | 1,322,427 | 0.3150× | 0.32× | ✅ |
| static | 25,850 | 41,003 | 0.6304× | 0.64× | ✅ |
| client | 18,611 | 19,729 | 0.9433× | 0.93× | ✅ |
| socket | 19,857 | 21,045 | 0.9435× | 0.93× | ✅ |
| pgsql | 22,824 | 24,662 | 0.9255× | 0.92× | ✅ |
| sqlite | 23,520 | 25,268 | 0.9308× | 0.93× | ✅ |
| curl | 86,134 | 90,396 | 0.9529× | 0.95× | ✅ |
| dns | 230,322 | 232,883 | 0.9890× | 0.99× | ✅ |
| file | 108,941 | 131,300 | 0.8297× | 0.83× | ✅ |

Geometric mean 0.8536×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
