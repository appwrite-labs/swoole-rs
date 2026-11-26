# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `5550ddd` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,719 | 15,210 | 0.9020× | 0.91× | ✅ |
| echo-1k | 16,758 | 18,162 | 0.9227× | 0.92× | ✅ |
| json | 22,051 | 23,164 | 0.9519× | 0.95× | ✅ |
| service-health | 16,308 | 17,773 | 0.9176× | 0.92× | ✅ |
| websocket | 6,740 | 8,189 | 0.8230× | 0.81× | ✅ |
| timer | 202,143 | 314,015 | 0.6437× | 0.65× | ✅ |
| app | 127,531 | 135,996 | 0.9378× | 0.93× | ✅ |
| http2 | 19,173 | 20,497 | 0.9354× | 0.92× | ✅ |
| table | 194,345 | 202,900 | 0.9578× | 0.96× | ✅ |
| channel | 137,023 | 155,717 | 0.8799× | 0.88× | ✅ |
| task | 32,071 | 33,272 | 0.9639× | 0.96× | ✅ |
| mysql | 50,290 | 51,088 | 0.9844× | 0.97× | ✅ |
| http-client | 33,656 | 35,796 | 0.9402× | 0.93× | ✅ |
| gzip | 194,294 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 180,021 | 181,573 | 0.9915× | 0.99× | ✅ |
| tcp-length | 4,662 | 4,763 | 0.9788× | 0.96× | ✅ |
| tcp-eof | 5,067 | 10,228 | 0.4954× | 0.5× | ✅ |
| udp | 7,616 | 7,986 | 0.9536× | 0.95× | ✅ |
| connect | 17,491 | 18,734 | 0.9336× | 0.93× | ✅ |
| https | 106,284 | 108,809 | 0.9768× | 0.98× | ✅ |
| wss | 101,561 | 104,800 | 0.9691× | 0.97× | ✅ |
| atomic | 50,423 | 51,285 | 0.9832× | 0.97× | ✅ |
| process-mode | 15,404 | 17,024 | 0.9048× | 0.89× | ✅ |
| http2-concurrent | 367,033 | 409,057 | 0.8973× | 0.89× | ✅ |
| websocket-64k | 90,035 | 159,694 | 0.5638× | 0.57× | ✅ |
| websocket-fragmented | 11,963 | 16,372 | 0.7307× | 0.72× | ✅ |
| upload | 418,910 | 1,322,429 | 0.3168× | 0.32× | ✅ |
| static | 25,352 | 40,999 | 0.6183× | 0.62× | ✅ |
| client | 18,596 | 19,730 | 0.9425× | 0.93× | ✅ |
| socket | 19,835 | 21,045 | 0.9425× | 0.93× | ✅ |
| pgsql | 22,887 | 24,662 | 0.9280× | 0.92× | ✅ |
| sqlite | 23,478 | 25,268 | 0.9291× | 0.93× | ✅ |
| curl | 86,078 | 90,551 | 0.9506× | 0.95× | ✅ |
| dns | 229,856 | 232,759 | 0.9875× | 0.99× | ✅ |
| file | 108,876 | 131,212 | 0.8298× | 0.83× | ✅ |

Geometric mean 0.8534×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
