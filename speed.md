# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `8dec066` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,740 | 15,214 | 0.9031× | 0.91× | ✅ |
| echo-1k | 16,756 | 18,165 | 0.9224× | 0.92× | ✅ |
| json | 22,076 | 23,173 | 0.9527× | 0.95× | ✅ |
| service-health | 16,331 | 17,786 | 0.9182× | 0.92× | ✅ |
| websocket | 6,676 | 8,189 | 0.8152× | 0.81× | ✅ |
| timer | 201,523 | 313,770 | 0.6423× | 0.65× | ✅ |
| app | 127,005 | 135,980 | 0.9340× | 0.93× | ✅ |
| http2 | 18,968 | 20,501 | 0.9252× | 0.92× | ✅ |
| table | 194,301 | 201,885 | 0.9624× | 0.96× | ✅ |
| channel | 137,353 | 155,726 | 0.8820× | 0.88× | ✅ |
| task | 31,960 | 33,268 | 0.9607× | 0.96× | ✅ |
| mysql | 50,042 | 51,085 | 0.9796× | 0.97× | ✅ |
| http-client | 33,365 | 35,783 | 0.9324× | 0.93× | ✅ |
| gzip | 194,378 | 197,095 | 0.9862× | 0.99× | ✅ |
| websocket-deflate | 179,104 | 181,562 | 0.9865× | 0.99× | ✅ |
| tcp-length | 4,646 | 4,763 | 0.9754× | 0.96× | ✅ |
| tcp-eof | 5,196 | 10,231 | 0.5079× | 0.5× | ✅ |
| udp | 7,561 | 7,986 | 0.9467× | 0.95× | ✅ |
| connect | 17,581 | 18,734 | 0.9384× | 0.93× | ✅ |
| https | 106,008 | 108,806 | 0.9743× | 0.98× | ✅ |
| wss | 101,212 | 104,803 | 0.9657× | 0.97× | ✅ |
| atomic | 50,187 | 51,285 | 0.9786× | 0.97× | ✅ |
| process-mode | 15,264 | 17,024 | 0.8966× | 0.89× | ✅ |
| http2-concurrent | 365,117 | 409,058 | 0.8926× | 0.89× | ✅ |
| websocket-64k | 89,969 | 159,692 | 0.5634× | 0.57× | ✅ |
| websocket-fragmented | 11,835 | 16,372 | 0.7229× | 0.72× | ✅ |
| upload | 416,435 | 1,322,432 | 0.3149× | 0.32× | ✅ |
| static | 27,115 | 41,063 | 0.6603× | 0.65× | ✅ |
| client | 18,473 | 19,730 | 0.9363× | 0.93× | ✅ |
| socket | 19,700 | 21,045 | 0.9361× | 0.93× | ✅ |
| pgsql | 23,090 | 24,663 | 0.9362× | 0.92× | ✅ |
| sqlite | 23,433 | 25,268 | 0.9274× | 0.93× | ✅ |
| curl | 85,923 | 90,423 | 0.9502× | 0.95× | ✅ |
| dns | 229,683 | 232,978 | 0.9859× | 0.99× | ✅ |
| file | 108,210 | 131,300 | 0.8241× | 0.83× | ✅ |

Geometric mean 0.8532×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
