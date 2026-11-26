# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `2ef2e58` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,755 | 15,214 | 0.9041× | 0.91× | ✅ |
| echo-1k | 16,781 | 18,165 | 0.9238× | 0.92× | ✅ |
| json | 22,071 | 23,164 | 0.9528× | 0.95× | ✅ |
| service-health | 16,325 | 17,777 | 0.9183× | 0.92× | ✅ |
| websocket | 6,723 | 8,189 | 0.8211× | 0.81× | ✅ |
| timer | 202,164 | 313,797 | 0.6443× | 0.65× | ✅ |
| app | 127,608 | 136,007 | 0.9382× | 0.93× | ✅ |
| http2 | 19,183 | 20,501 | 0.9357× | 0.92× | ✅ |
| table | 194,365 | 202,900 | 0.9579× | 0.96× | ✅ |
| channel | 137,231 | 155,714 | 0.8813× | 0.88× | ✅ |
| task | 32,099 | 33,269 | 0.9649× | 0.96× | ✅ |
| mysql | 50,315 | 51,084 | 0.9849× | 0.97× | ✅ |
| http-client | 33,678 | 35,793 | 0.9409× | 0.93× | ✅ |
| gzip | 194,276 | 197,099 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,467 | 181,573 | 0.9884× | 0.99× | ✅ |
| tcp-length | 4,662 | 4,763 | 0.9788× | 0.96× | ✅ |
| tcp-eof | 5,067 | 10,231 | 0.4952× | 0.5× | ✅ |
| udp | 7,616 | 7,987 | 0.9536× | 0.95× | ✅ |
| connect | 17,522 | 18,734 | 0.9353× | 0.93× | ✅ |
| https | 106,267 | 108,802 | 0.9767× | 0.98× | ✅ |
| wss | 101,560 | 104,797 | 0.9691× | 0.97× | ✅ |
| atomic | 50,440 | 51,285 | 0.9835× | 0.97× | ✅ |
| process-mode | 15,434 | 17,025 | 0.9066× | 0.89× | ✅ |
| http2-concurrent | 366,437 | 409,053 | 0.8958× | 0.89× | ✅ |
| websocket-64k | 90,001 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,925 | 16,368 | 0.7285× | 0.72× | ✅ |
| upload | 418,883 | 1,322,430 | 0.3168× | 0.32× | ✅ |
| static | 25,229 | 41,092 | 0.6140× | 0.62× | ✅ |
| client | 18,617 | 19,730 | 0.9436× | 0.93× | ✅ |
| socket | 19,856 | 21,045 | 0.9435× | 0.93× | ✅ |
| pgsql | 22,913 | 24,659 | 0.9292× | 0.92× | ✅ |
| sqlite | 23,506 | 25,268 | 0.9303× | 0.93× | ✅ |
| curl | 86,109 | 90,419 | 0.9523× | 0.95× | ✅ |
| dns | 229,980 | 232,885 | 0.9875× | 0.99× | ✅ |
| file | 108,904 | 130,918 | 0.8318× | 0.83× | ✅ |

Geometric mean 0.8536×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
