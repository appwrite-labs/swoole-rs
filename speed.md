# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `433acde` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,746 | 15,210 | 0.9037× | 0.91× | ✅ |
| echo-1k | 16,785 | 18,165 | 0.9240× | 0.92× | ✅ |
| json | 22,079 | 23,173 | 0.9528× | 0.95× | ✅ |
| service-health | 16,334 | 17,786 | 0.9183× | 0.92× | ✅ |
| websocket | 6,723 | 8,189 | 0.8211× | 0.81× | ✅ |
| timer | 202,166 | 313,970 | 0.6439× | 0.65× | ✅ |
| app | 127,617 | 135,972 | 0.9386× | 0.93× | ✅ |
| http2 | 19,175 | 20,501 | 0.9353× | 0.92× | ✅ |
| table | 194,306 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,240 | 155,726 | 0.8813× | 0.88× | ✅ |
| task | 32,112 | 33,269 | 0.9652× | 0.96× | ✅ |
| mysql | 50,315 | 51,082 | 0.9850× | 0.97× | ✅ |
| http-client | 33,687 | 35,788 | 0.9413× | 0.93× | ✅ |
| gzip | 194,276 | 197,099 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,467 | 181,573 | 0.9884× | 0.99× | ✅ |
| tcp-length | 4,662 | 4,766 | 0.9781× | 0.96× | ✅ |
| tcp-eof | 5,067 | 10,231 | 0.4952× | 0.5× | ✅ |
| udp | 7,616 | 7,986 | 0.9536× | 0.95× | ✅ |
| connect | 17,522 | 18,734 | 0.9353× | 0.93× | ✅ |
| https | 106,276 | 108,809 | 0.9767× | 0.98× | ✅ |
| wss | 101,560 | 104,804 | 0.9690× | 0.97× | ✅ |
| atomic | 50,440 | 51,276 | 0.9837× | 0.97× | ✅ |
| process-mode | 15,431 | 17,025 | 0.9063× | 0.89× | ✅ |
| http2-concurrent | 368,453 | 408,864 | 0.9012× | 0.89× | ✅ |
| websocket-64k | 90,000 | 159,696 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,928 | 16,368 | 0.7288× | 0.72× | ✅ |
| upload | 418,895 | 1,322,425 | 0.3168× | 0.32× | ✅ |
| static | 25,373 | 41,063 | 0.6179× | 0.62× | ✅ |
| client | 18,620 | 19,721 | 0.9441× | 0.93× | ✅ |
| socket | 19,856 | 21,036 | 0.9439× | 0.93× | ✅ |
| pgsql | 22,915 | 24,662 | 0.9292× | 0.92× | ✅ |
| sqlite | 23,506 | 25,268 | 0.9303× | 0.93× | ✅ |
| curl | 86,118 | 90,422 | 0.9524× | 0.95× | ✅ |
| dns | 230,176 | 232,864 | 0.9885× | 0.99× | ✅ |
| file | 108,904 | 130,918 | 0.8318× | 0.83× | ✅ |

Geometric mean 0.8540×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
