# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `e5ee378` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,755 | 15,210 | 0.9043× | 0.91× | ✅ |
| echo-1k | 16,785 | 18,166 | 0.9240× | 0.92× | ✅ |
| json | 22,070 | 23,173 | 0.9524× | 0.95× | ✅ |
| service-health | 16,325 | 17,786 | 0.9178× | 0.92× | ✅ |
| websocket | 6,723 | 8,189 | 0.8211× | 0.81× | ✅ |
| timer | 202,283 | 313,892 | 0.6444× | 0.65× | ✅ |
| app | 127,623 | 135,938 | 0.9388× | 0.93× | ✅ |
| http2 | 19,173 | 20,501 | 0.9352× | 0.92× | ✅ |
| table | 194,297 | 201,885 | 0.9624× | 0.96× | ✅ |
| channel | 137,231 | 155,726 | 0.8812× | 0.88× | ✅ |
| task | 32,108 | 33,272 | 0.9650× | 0.96× | ✅ |
| mysql | 50,311 | 51,088 | 0.9848× | 0.97× | ✅ |
| http-client | 33,678 | 35,793 | 0.9409× | 0.93× | ✅ |
| gzip | 194,272 | 197,095 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,987 | 181,570 | 0.9913× | 0.99× | ✅ |
| tcp-length | 4,662 | 4,766 | 0.9781× | 0.96× | ✅ |
| tcp-eof | 5,067 | 10,231 | 0.4952× | 0.5× | ✅ |
| udp | 7,616 | 7,986 | 0.9536× | 0.95× | ✅ |
| connect | 17,525 | 18,730 | 0.9357× | 0.93× | ✅ |
| https | 106,267 | 108,809 | 0.9766× | 0.98× | ✅ |
| wss | 101,560 | 104,803 | 0.9690× | 0.97× | ✅ |
| atomic | 50,440 | 51,285 | 0.9835× | 0.97× | ✅ |
| process-mode | 15,434 | 17,024 | 0.9066× | 0.89× | ✅ |
| http2-concurrent | 365,335 | 409,056 | 0.8931× | 0.89× | ✅ |
| websocket-64k | 90,010 | 159,692 | 0.5637× | 0.57× | ✅ |
| websocket-fragmented | 11,928 | 16,368 | 0.7287× | 0.72× | ✅ |
| upload | 418,886 | 1,322,429 | 0.3168× | 0.32× | ✅ |
| static | 25,229 | 41,092 | 0.6140× | 0.62× | ✅ |
| client | 18,617 | 19,729 | 0.9436× | 0.93× | ✅ |
| socket | 19,857 | 21,042 | 0.9437× | 0.93× | ✅ |
| pgsql | 22,916 | 24,662 | 0.9292× | 0.92× | ✅ |
| sqlite | 23,506 | 25,268 | 0.9303× | 0.93× | ✅ |
| curl | 86,108 | 90,394 | 0.9526× | 0.95× | ✅ |
| dns | 230,260 | 232,660 | 0.9897× | 0.99× | ✅ |
| file | 108,904 | 131,297 | 0.8294× | 0.83× | ✅ |

Geometric mean 0.8537×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
