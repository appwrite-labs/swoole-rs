# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `5550ddd` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,719 | 15,214 | 0.9017× | 0.91× | ✅ |
| echo-1k | 16,758 | 18,162 | 0.9227× | 0.92× | ✅ |
| json | 22,050 | 23,170 | 0.9517× | 0.95× | ✅ |
| service-health | 16,308 | 17,786 | 0.9169× | 0.92× | ✅ |
| websocket | 6,739 | 8,189 | 0.8230× | 0.81× | ✅ |
| timer | 202,238 | 313,982 | 0.6441× | 0.65× | ✅ |
| app | 127,532 | 135,961 | 0.9380× | 0.93× | ✅ |
| http2 | 19,178 | 20,500 | 0.9355× | 0.92× | ✅ |
| table | 194,281 | 201,885 | 0.9623× | 0.96× | ✅ |
| channel | 137,019 | 155,726 | 0.8799× | 0.88× | ✅ |
| task | 32,073 | 33,272 | 0.9640× | 0.96× | ✅ |
| mysql | 50,291 | 51,085 | 0.9845× | 0.97× | ✅ |
| http-client | 33,653 | 35,789 | 0.9403× | 0.93× | ✅ |
| gzip | 194,290 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 180,025 | 181,573 | 0.9915× | 0.99× | ✅ |
| tcp-length | 4,662 | 4,766 | 0.9781× | 0.96× | ✅ |
| tcp-eof | 5,067 | 10,231 | 0.4952× | 0.5× | ✅ |
| udp | 7,616 | 7,986 | 0.9536× | 0.95× | ✅ |
| connect | 17,495 | 18,734 | 0.9338× | 0.93× | ✅ |
| https | 106,288 | 108,806 | 0.9769× | 0.98× | ✅ |
| wss | 101,572 | 104,797 | 0.9692× | 0.97× | ✅ |
| atomic | 50,423 | 51,285 | 0.9832× | 0.97× | ✅ |
| process-mode | 15,406 | 17,021 | 0.9051× | 0.89× | ✅ |
| http2-concurrent | 366,920 | 409,053 | 0.8970× | 0.89× | ✅ |
| websocket-64k | 90,033 | 159,691 | 0.5638× | 0.57× | ✅ |
| websocket-fragmented | 11,963 | 16,368 | 0.7309× | 0.72× | ✅ |
| upload | 418,892 | 1,322,429 | 0.3168× | 0.32× | ✅ |
| static | 25,393 | 40,999 | 0.6193× | 0.62× | ✅ |
| client | 18,596 | 19,730 | 0.9425× | 0.93× | ✅ |
| socket | 19,835 | 21,042 | 0.9427× | 0.93× | ✅ |
| pgsql | 22,906 | 24,662 | 0.9288× | 0.92× | ✅ |
| sqlite | 23,478 | 25,268 | 0.9291× | 0.93× | ✅ |
| curl | 86,077 | 90,354 | 0.9527× | 0.95× | ✅ |
| dns | 230,233 | 232,770 | 0.9891× | 0.99× | ✅ |
| file | 108,880 | 130,918 | 0.8317× | 0.83× | ✅ |

Geometric mean 0.8537×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
