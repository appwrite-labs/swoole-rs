# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `1c9c963` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,481 | 15,210 | 0.9521× | 0.94× | ✅ |
| echo-1k | 17,625 | 18,165 | 0.9703× | 0.97× | ✅ |
| json | 22,584 | 23,170 | 0.9747× | 0.97× | ✅ |
| service-health | 17,067 | 17,786 | 0.9596× | 0.95× | ✅ |
| websocket | 6,908 | 8,184 | 0.8440× | 0.84× | ✅ |
| timer | 202,792 | 314,373 | 0.6451× | 0.65× | ✅ |
| app | 126,739 | 135,861 | 0.9329× | 0.94× | ✅ |
| http2 | 20,440 | 20,464 | 0.9988× | 0.99× | ✅ |
| table | 196,353 | 201,874 | 0.9727× | 0.98× | ✅ |
| channel | 139,771 | 154,667 | 0.9037× | 0.91× | ✅ |
| task | 32,459 | 33,268 | 0.9757× | 0.97× | ✅ |
| mysql | 50,115 | 51,060 | 0.9815× | 0.98× | ✅ |
| http-client | 34,319 | 35,833 | 0.9578× | 0.96× | ✅ |
| client | 18,979 | 19,766 | 0.9602× | 0.95× | ✅ |
| socket | 20,448 | 21,070 | 0.9705× | 0.97× | ✅ |
| gzip | 196,484 | 197,080 | 0.9970× | 1.0× | ✅ |
| websocket-deflate | 181,686 | 181,573 | 1.0006× | 1.0× | ❌ |
| tcp-length | 5,441 | 4,762 | 1.1426× | 1.13× | ❌ |
| tcp-eof | 5,934 | 10,228 | 0.5802× | 0.58× | ✅ |
| udp | 8,018 | 7,987 | 1.0039× | 1.0× | ❌ |
| connect | 18,930 | 18,703 | 1.0121× | 1.03× | ➖ |
| https | 109,690 | 108,810 | 1.0081× | 1.01× | ➖ |
| wss | 105,091 | 104,802 | 1.0028× | 1.0× | ❌ |
| atomic | 50,287 | 51,325 | 0.9798× | 0.98× | ✅ |
| process-mode | 16,771 | 16,959 | 0.9889× | 0.98× | ✅ |
| http2-concurrent | 379,473 | 409,072 | 0.9276× | 0.92× | ✅ |
| websocket-64k | 156,353 | 159,702 | 0.9790× | 0.98× | ✅ |
| websocket-fragmented | 12,881 | 16,339 | 0.7884× | 0.79× | ✅ |
| upload | 675,626 | 1,322,380 | 0.5109× | 0.51× | ✅ |
| static | 34,784 | 41,074 | 0.8469× | 0.86× | ✅ |
| pgsql | 23,779 | 24,594 | 0.9669× | 0.97× | ✅ |
| sqlite | 24,084 | 25,232 | 0.9545× | 0.96× | ✅ |
| curl | 86,442 | 90,511 | 0.9550× | 0.96× | ✅ |
| dns | 231,190 | 232,984 | 0.9923× | 1.0× | ✅ |

Geometric mean 0.9200×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
