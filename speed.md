# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `341aeae` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,715 | 15,213 | 0.9015× | 0.91× | ✅ |
| echo-1k | 16,758 | 18,165 | 0.9225× | 0.92× | ✅ |
| json | 22,051 | 23,164 | 0.9519× | 0.95× | ✅ |
| service-health | 16,305 | 17,777 | 0.9172× | 0.92× | ✅ |
| websocket | 6,736 | 8,189 | 0.8226× | 0.81× | ✅ |
| timer | 202,148 | 313,777 | 0.6442× | 0.65× | ✅ |
| app | 127,581 | 135,961 | 0.9384× | 0.93× | ✅ |
| http2 | 19,182 | 20,500 | 0.9357× | 0.92× | ✅ |
| table | 194,345 | 202,900 | 0.9578× | 0.96× | ✅ |
| channel | 137,019 | 155,717 | 0.8799× | 0.88× | ✅ |
| task | 32,073 | 33,272 | 0.9640× | 0.96× | ✅ |
| mysql | 50,290 | 51,088 | 0.9844× | 0.97× | ✅ |
| http-client | 33,657 | 35,797 | 0.9402× | 0.93× | ✅ |
| gzip | 194,290 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 180,021 | 181,570 | 0.9915× | 0.99× | ✅ |
| tcp-length | 4,662 | 4,763 | 0.9788× | 0.96× | ✅ |
| tcp-eof | 5,067 | 10,231 | 0.4952× | 0.5× | ✅ |
| udp | 7,616 | 7,986 | 0.9536× | 0.95× | ✅ |
| connect | 17,494 | 18,734 | 0.9338× | 0.93× | ✅ |
| https | 106,284 | 108,813 | 0.9768× | 0.98× | ✅ |
| wss | 101,570 | 104,800 | 0.9692× | 0.97× | ✅ |
| atomic | 50,423 | 51,285 | 0.9832× | 0.97× | ✅ |
| process-mode | 15,404 | 17,027 | 0.9046× | 0.89× | ✅ |
| http2-concurrent | 366,998 | 409,057 | 0.8972× | 0.89× | ✅ |
| websocket-64k | 90,036 | 159,691 | 0.5638× | 0.57× | ✅ |
| websocket-fragmented | 11,962 | 16,372 | 0.7307× | 0.72× | ✅ |
| upload | 418,893 | 1,322,430 | 0.3168× | 0.32× | ✅ |
| static | 25,457 | 41,092 | 0.6195× | 0.62× | ✅ |
| client | 18,596 | 19,730 | 0.9425× | 0.93× | ✅ |
| socket | 19,835 | 21,046 | 0.9425× | 0.93× | ✅ |
| pgsql | 22,891 | 24,663 | 0.9281× | 0.92× | ✅ |
| sqlite | 23,478 | 25,268 | 0.9291× | 0.93× | ✅ |
| curl | 86,077 | 90,546 | 0.9507× | 0.95× | ✅ |
| dns | 229,937 | 232,947 | 0.9871× | 0.99× | ✅ |
| file | 108,876 | 131,300 | 0.8292× | 0.83× | ✅ |

Geometric mean 0.8535×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
