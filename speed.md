# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `b25485e` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,739 | 15,213 | 0.9031× | 0.91× | ✅ |
| echo-1k | 16,756 | 18,165 | 0.9224× | 0.92× | ✅ |
| json | 22,073 | 23,173 | 0.9525× | 0.95× | ✅ |
| service-health | 16,331 | 17,786 | 0.9182× | 0.92× | ✅ |
| websocket | 6,671 | 8,189 | 0.8147× | 0.81× | ✅ |
| timer | 201,486 | 313,842 | 0.6420× | 0.65× | ✅ |
| app | 126,982 | 135,932 | 0.9342× | 0.93× | ✅ |
| http2 | 18,969 | 20,501 | 0.9253× | 0.92× | ✅ |
| table | 194,301 | 201,885 | 0.9624× | 0.96× | ✅ |
| channel | 137,353 | 155,726 | 0.8820× | 0.88× | ✅ |
| task | 31,956 | 33,268 | 0.9606× | 0.96× | ✅ |
| mysql | 50,050 | 51,076 | 0.9799× | 0.97× | ✅ |
| http-client | 33,362 | 35,797 | 0.9320× | 0.93× | ✅ |
| gzip | 194,366 | 197,099 | 0.9861× | 0.99× | ✅ |
| websocket-deflate | 179,107 | 181,573 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,646 | 4,763 | 0.9754× | 0.96× | ✅ |
| tcp-eof | 5,196 | 10,231 | 0.5079× | 0.5× | ✅ |
| udp | 7,561 | 7,986 | 0.9467× | 0.95× | ✅ |
| connect | 17,577 | 18,734 | 0.9382× | 0.93× | ✅ |
| https | 106,004 | 108,809 | 0.9742× | 0.98× | ✅ |
| wss | 101,216 | 104,797 | 0.9658× | 0.97× | ✅ |
| atomic | 50,187 | 51,285 | 0.9786× | 0.97× | ✅ |
| process-mode | 15,267 | 17,024 | 0.8968× | 0.89× | ✅ |
| http2-concurrent | 366,493 | 409,053 | 0.8960× | 0.89× | ✅ |
| websocket-64k | 89,973 | 159,691 | 0.5634× | 0.57× | ✅ |
| websocket-fragmented | 11,835 | 16,372 | 0.7229× | 0.72× | ✅ |
| upload | 416,432 | 1,322,429 | 0.3149× | 0.32× | ✅ |
| static | 27,115 | 41,063 | 0.6603× | 0.65× | ✅ |
| client | 18,476 | 19,729 | 0.9365× | 0.93× | ✅ |
| socket | 19,704 | 21,046 | 0.9362× | 0.93× | ✅ |
| pgsql | 23,090 | 24,662 | 0.9362× | 0.92× | ✅ |
| sqlite | 23,433 | 25,268 | 0.9274× | 0.93× | ✅ |
| curl | 85,922 | 90,546 | 0.9489× | 0.95× | ✅ |
| dns | 229,709 | 232,826 | 0.9866× | 0.99× | ✅ |
| file | 108,210 | 130,841 | 0.8270× | 0.83× | ✅ |

Geometric mean 0.8533×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
