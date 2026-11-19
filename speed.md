# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `27400c0` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,744 | 15,210 | 0.9036× | 0.91× | ✅ |
| echo-1k | 16,756 | 18,165 | 0.9224× | 0.92× | ✅ |
| json | 22,073 | 23,173 | 0.9525× | 0.95× | ✅ |
| service-health | 16,331 | 17,786 | 0.9182× | 0.92× | ✅ |
| websocket | 6,671 | 8,189 | 0.8147× | 0.81× | ✅ |
| timer | 201,542 | 313,797 | 0.6423× | 0.65× | ✅ |
| app | 127,049 | 135,977 | 0.9343× | 0.93× | ✅ |
| http2 | 18,969 | 20,497 | 0.9255× | 0.92× | ✅ |
| table | 194,301 | 201,885 | 0.9624× | 0.96× | ✅ |
| channel | 137,353 | 155,723 | 0.8820× | 0.88× | ✅ |
| task | 31,957 | 33,269 | 0.9606× | 0.96× | ✅ |
| mysql | 50,047 | 51,088 | 0.9796× | 0.97× | ✅ |
| http-client | 33,359 | 35,794 | 0.9320× | 0.93× | ✅ |
| gzip | 194,370 | 197,099 | 0.9862× | 0.99× | ✅ |
| websocket-deflate | 179,107 | 181,573 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,646 | 4,763 | 0.9754× | 0.96× | ✅ |
| tcp-eof | 5,196 | 10,231 | 0.5079× | 0.5× | ✅ |
| udp | 7,560 | 7,986 | 0.9466× | 0.95× | ✅ |
| connect | 17,581 | 18,734 | 0.9384× | 0.93× | ✅ |
| https | 106,004 | 108,809 | 0.9742× | 0.98× | ✅ |
| wss | 101,216 | 104,797 | 0.9658× | 0.97× | ✅ |
| atomic | 50,184 | 51,285 | 0.9785× | 0.97× | ✅ |
| process-mode | 15,267 | 17,027 | 0.8966× | 0.89× | ✅ |
| http2-concurrent | 365,014 | 409,056 | 0.8923× | 0.89× | ✅ |
| websocket-64k | 89,974 | 159,694 | 0.5634× | 0.57× | ✅ |
| websocket-fragmented | 11,839 | 16,368 | 0.7233× | 0.72× | ✅ |
| upload | 416,435 | 1,322,429 | 0.3149× | 0.32× | ✅ |
| static | 27,057 | 41,005 | 0.6598× | 0.65× | ✅ |
| client | 18,478 | 19,729 | 0.9366× | 0.93× | ✅ |
| socket | 19,705 | 21,042 | 0.9365× | 0.93× | ✅ |
| pgsql | 23,087 | 24,662 | 0.9361× | 0.92× | ✅ |
| sqlite | 23,433 | 25,268 | 0.9274× | 0.93× | ✅ |
| curl | 85,922 | 90,418 | 0.9503× | 0.95× | ✅ |
| dns | 229,791 | 232,538 | 0.9882× | 0.99× | ✅ |
| file | 108,210 | 131,296 | 0.8242× | 0.83× | ✅ |

Geometric mean 0.8532×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
