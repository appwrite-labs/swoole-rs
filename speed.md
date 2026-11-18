# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `be72455` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,737 | 15,210 | 0.9031× | 0.91× | ✅ |
| echo-1k | 16,736 | 18,165 | 0.9213× | 0.92× | ✅ |
| json | 22,061 | 23,173 | 0.9520× | 0.95× | ✅ |
| service-health | 16,308 | 17,782 | 0.9171× | 0.92× | ✅ |
| websocket | 6,676 | 8,192 | 0.8149× | 0.8× | ✅ |
| timer | 201,495 | 313,774 | 0.6422× | 0.65× | ✅ |
| app | 126,677 | 135,888 | 0.9322× | 0.93× | ✅ |
| http2 | 18,995 | 20,496 | 0.9268× | 0.92× | ✅ |
| table | 193,897 | 201,885 | 0.9604× | 0.96× | ✅ |
| channel | 138,483 | 155,726 | 0.8893× | 0.88× | ✅ |
| task | 31,943 | 33,268 | 0.9602× | 0.96× | ✅ |
| mysql | 49,801 | 51,061 | 0.9753× | 0.97× | ✅ |
| http-client | 33,593 | 35,796 | 0.9385× | 0.93× | ✅ |
| gzip | 194,354 | 197,095 | 0.9861× | 0.99× | ✅ |
| websocket-deflate | 179,099 | 181,570 | 0.9864× | 0.99× | ✅ |
| tcp-length | 4,644 | 4,766 | 0.9743× | 0.96× | ✅ |
| tcp-eof | 5,194 | 10,228 | 0.5078× | 0.5× | ✅ |
| udp | 7,583 | 7,986 | 0.9495× | 0.95× | ✅ |
| connect | 17,567 | 18,734 | 0.9377× | 0.93× | ✅ |
| https | 106,017 | 108,802 | 0.9744× | 0.98× | ✅ |
| wss | 101,243 | 104,797 | 0.9661× | 0.97× | ✅ |
| atomic | 50,164 | 51,285 | 0.9781× | 0.97× | ✅ |
| process-mode | 15,261 | 17,025 | 0.8964× | 0.89× | ✅ |
| http2-concurrent | 364,735 | 409,057 | 0.8916× | 0.89× | ✅ |
| websocket-64k | 89,976 | 159,694 | 0.5634× | 0.57× | ✅ |
| websocket-fragmented | 11,844 | 16,368 | 0.7236× | 0.72× | ✅ |
| upload | 416,234 | 1,322,425 | 0.3148× | 0.32× | ✅ |
| static | 27,052 | 41,005 | 0.6597× | 0.65× | ✅ |
| client | 18,360 | 19,731 | 0.9305× | 0.93× | ✅ |
| socket | 19,591 | 21,046 | 0.9309× | 0.93× | ✅ |
| pgsql | 22,584 | 24,618 | 0.9174× | 0.92× | ✅ |
| sqlite | 23,401 | 25,236 | 0.9273× | 0.93× | ✅ |
| curl | 85,347 | 90,313 | 0.9450× | 0.95× | ✅ |
| dns | 229,458 | 232,543 | 0.9867× | 0.99× | ✅ |

Geometric mean 0.8532×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
