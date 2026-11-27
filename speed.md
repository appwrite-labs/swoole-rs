# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `341aeae` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,719 | 15,214 | 0.9017× | 0.91× | ✅ |
| echo-1k | 16,758 | 18,165 | 0.9225× | 0.92× | ✅ |
| json | 22,047 | 23,164 | 0.9518× | 0.95× | ✅ |
| service-health | 16,308 | 17,777 | 0.9174× | 0.92× | ✅ |
| websocket | 6,736 | 8,192 | 0.8223× | 0.81× | ✅ |
| timer | 202,151 | 313,770 | 0.6443× | 0.65× | ✅ |
| app | 127,587 | 135,944 | 0.9385× | 0.93× | ✅ |
| http2 | 19,169 | 20,500 | 0.9351× | 0.92× | ✅ |
| table | 194,341 | 202,900 | 0.9578× | 0.96× | ✅ |
| channel | 137,019 | 155,717 | 0.8799× | 0.88× | ✅ |
| task | 32,073 | 33,272 | 0.9640× | 0.96× | ✅ |
| mysql | 50,287 | 51,088 | 0.9843× | 0.97× | ✅ |
| http-client | 33,653 | 35,792 | 0.9402× | 0.93× | ✅ |
| gzip | 194,290 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 180,025 | 181,573 | 0.9915× | 0.99× | ✅ |
| tcp-length | 4,662 | 4,763 | 0.9788× | 0.96× | ✅ |
| tcp-eof | 5,067 | 10,228 | 0.4954× | 0.5× | ✅ |
| udp | 7,616 | 7,987 | 0.9536× | 0.95× | ✅ |
| connect | 17,494 | 18,734 | 0.9338× | 0.93× | ✅ |
| https | 106,284 | 108,809 | 0.9768× | 0.98× | ✅ |
| wss | 101,570 | 104,797 | 0.9692× | 0.97× | ✅ |
| atomic | 50,423 | 51,285 | 0.9832× | 0.97× | ✅ |
| process-mode | 15,404 | 17,027 | 0.9046× | 0.89× | ✅ |
| http2-concurrent | 366,892 | 409,057 | 0.8969× | 0.89× | ✅ |
| websocket-64k | 90,033 | 159,694 | 0.5638× | 0.57× | ✅ |
| websocket-fragmented | 11,966 | 16,368 | 0.7311× | 0.72× | ✅ |
| upload | 418,896 | 1,322,429 | 0.3168× | 0.32× | ✅ |
| static | 25,393 | 41,028 | 0.6189× | 0.62× | ✅ |
| client | 18,596 | 19,731 | 0.9425× | 0.93× | ✅ |
| socket | 19,835 | 21,045 | 0.9425× | 0.93× | ✅ |
| pgsql | 22,888 | 24,664 | 0.9280× | 0.92× | ✅ |
| sqlite | 23,478 | 25,268 | 0.9291× | 0.93× | ✅ |
| curl | 86,076 | 90,547 | 0.9506× | 0.95× | ✅ |
| dns | 229,971 | 232,550 | 0.9889× | 0.99× | ✅ |
| file | 108,880 | 131,297 | 0.8293× | 0.83× | ✅ |

Geometric mean 0.8535×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
