# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `ebd5d62` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,755 | 15,213 | 0.9041× | 0.91× | ✅ |
| echo-1k | 16,797 | 18,165 | 0.9247× | 0.92× | ✅ |
| json | 22,080 | 23,173 | 0.9528× | 0.95× | ✅ |
| service-health | 16,338 | 17,786 | 0.9186× | 0.92× | ✅ |
| websocket | 6,721 | 8,189 | 0.8208× | 0.81× | ✅ |
| timer | 202,107 | 313,775 | 0.6441× | 0.65× | ✅ |
| app | 127,641 | 135,965 | 0.9388× | 0.93× | ✅ |
| http2 | 18,959 | 20,495 | 0.9251× | 0.92× | ✅ |
| table | 194,306 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,362 | 155,726 | 0.8821× | 0.88× | ✅ |
| task | 32,086 | 33,268 | 0.9645× | 0.96× | ✅ |
| mysql | 50,311 | 51,085 | 0.9849× | 0.97× | ✅ |
| http-client | 33,626 | 35,796 | 0.9394× | 0.93× | ✅ |
| gzip | 194,287 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,570 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,766 | 0.9779× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,986 | 0.9526× | 0.95× | ✅ |
| connect | 17,532 | 18,734 | 0.9359× | 0.93× | ✅ |
| https | 106,310 | 108,809 | 0.9770× | 0.98× | ✅ |
| wss | 101,599 | 104,797 | 0.9695× | 0.97× | ✅ |
| atomic | 50,196 | 51,272 | 0.9790× | 0.97× | ✅ |
| process-mode | 15,416 | 17,028 | 0.9053× | 0.89× | ✅ |
| http2-concurrent | 364,631 | 408,861 | 0.8918× | 0.89× | ✅ |
| websocket-64k | 90,004 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,927 | 16,369 | 0.7287× | 0.72× | ✅ |
| upload | 416,528 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,854 | 41,034 | 0.6301× | 0.64× | ✅ |
| client | 18,620 | 19,721 | 0.9442× | 0.93× | ✅ |
| socket | 19,860 | 21,037 | 0.9441× | 0.93× | ✅ |
| pgsql | 22,826 | 24,663 | 0.9255× | 0.92× | ✅ |
| sqlite | 23,527 | 25,268 | 0.9311× | 0.93× | ✅ |
| curl | 86,130 | 90,422 | 0.9525× | 0.95× | ✅ |
| dns | 230,339 | 232,909 | 0.9890× | 0.99× | ✅ |
| file | 109,092 | 131,300 | 0.8309× | 0.83× | ✅ |

Geometric mean 0.8536×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
