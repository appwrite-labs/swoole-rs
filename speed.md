# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `3e82df0` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,752 | 15,214 | 0.9039× | 0.91× | ✅ |
| echo-1k | 16,760 | 18,165 | 0.9226× | 0.92× | ✅ |
| json | 22,073 | 23,174 | 0.9525× | 0.95× | ✅ |
| service-health | 16,331 | 17,786 | 0.9182× | 0.92× | ✅ |
| websocket | 6,671 | 8,189 | 0.8147× | 0.81× | ✅ |
| timer | 201,616 | 313,767 | 0.6426× | 0.65× | ✅ |
| app | 127,012 | 135,937 | 0.9343× | 0.93× | ✅ |
| http2 | 18,971 | 20,501 | 0.9254× | 0.92× | ✅ |
| table | 194,301 | 201,885 | 0.9624× | 0.96× | ✅ |
| channel | 137,353 | 155,726 | 0.8820× | 0.88× | ✅ |
| task | 31,960 | 33,268 | 0.9607× | 0.96× | ✅ |
| mysql | 50,054 | 51,085 | 0.9798× | 0.97× | ✅ |
| http-client | 33,370 | 35,784 | 0.9325× | 0.93× | ✅ |
| gzip | 194,378 | 197,095 | 0.9862× | 0.99× | ✅ |
| websocket-deflate | 179,107 | 181,565 | 0.9865× | 0.99× | ✅ |
| tcp-length | 4,646 | 4,766 | 0.9748× | 0.96× | ✅ |
| tcp-eof | 5,196 | 10,231 | 0.5079× | 0.5× | ✅ |
| udp | 7,560 | 7,986 | 0.9466× | 0.95× | ✅ |
| connect | 17,588 | 18,734 | 0.9388× | 0.93× | ✅ |
| https | 106,004 | 108,802 | 0.9743× | 0.98× | ✅ |
| wss | 101,216 | 104,797 | 0.9658× | 0.97× | ✅ |
| atomic | 50,187 | 51,281 | 0.9787× | 0.97× | ✅ |
| process-mode | 15,272 | 17,021 | 0.8972× | 0.89× | ✅ |
| http2-concurrent | 364,824 | 409,053 | 0.8919× | 0.89× | ✅ |
| websocket-64k | 89,970 | 159,692 | 0.5634× | 0.57× | ✅ |
| websocket-fragmented | 11,835 | 16,368 | 0.7231× | 0.72× | ✅ |
| upload | 416,434 | 1,322,430 | 0.3149× | 0.32× | ✅ |
| static | 27,144 | 41,092 | 0.6606× | 0.65× | ✅ |
| client | 18,477 | 19,731 | 0.9364× | 0.93× | ✅ |
| socket | 19,704 | 21,046 | 0.9363× | 0.93× | ✅ |
| pgsql | 23,091 | 24,662 | 0.9363× | 0.92× | ✅ |
| sqlite | 23,433 | 25,268 | 0.9274× | 0.93× | ✅ |
| curl | 85,396 | 90,418 | 0.9445× | 0.95× | ✅ |
| dns | 229,639 | 233,137 | 0.9850× | 0.99× | ✅ |
| file | 108,189 | 130,918 | 0.8264× | 0.83× | ✅ |

Geometric mean 0.8531×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
