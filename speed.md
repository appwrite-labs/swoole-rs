# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `341aeae` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,715 | 15,214 | 0.9015× | 0.91× | ✅ |
| echo-1k | 16,758 | 18,162 | 0.9227× | 0.92× | ✅ |
| json | 22,050 | 23,164 | 0.9519× | 0.95× | ✅ |
| service-health | 16,305 | 17,777 | 0.9172× | 0.92× | ✅ |
| websocket | 6,736 | 8,186 | 0.8229× | 0.81× | ✅ |
| timer | 202,149 | 313,985 | 0.6438× | 0.65× | ✅ |
| app | 127,567 | 135,909 | 0.9386× | 0.93× | ✅ |
| http2 | 19,157 | 20,500 | 0.9345× | 0.92× | ✅ |
| table | 194,345 | 202,900 | 0.9578× | 0.96× | ✅ |
| channel | 137,019 | 155,717 | 0.8799× | 0.88× | ✅ |
| task | 32,076 | 33,269 | 0.9641× | 0.96× | ✅ |
| mysql | 50,290 | 51,088 | 0.9844× | 0.97× | ✅ |
| http-client | 33,665 | 35,784 | 0.9408× | 0.93× | ✅ |
| gzip | 194,276 | 197,095 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,984 | 181,562 | 0.9913× | 0.99× | ✅ |
| tcp-length | 4,662 | 4,763 | 0.9788× | 0.96× | ✅ |
| tcp-eof | 5,067 | 10,231 | 0.4952× | 0.5× | ✅ |
| udp | 7,617 | 7,986 | 0.9537× | 0.95× | ✅ |
| connect | 17,494 | 18,734 | 0.9338× | 0.93× | ✅ |
| https | 106,288 | 108,806 | 0.9769× | 0.98× | ✅ |
| wss | 101,574 | 104,797 | 0.9692× | 0.97× | ✅ |
| atomic | 50,423 | 51,281 | 0.9833× | 0.97× | ✅ |
| process-mode | 15,404 | 17,024 | 0.9048× | 0.89× | ✅ |
| http2-concurrent | 366,929 | 409,053 | 0.8970× | 0.89× | ✅ |
| websocket-64k | 90,034 | 159,691 | 0.5638× | 0.57× | ✅ |
| websocket-fragmented | 11,963 | 16,368 | 0.7309× | 0.72× | ✅ |
| upload | 418,907 | 1,322,429 | 0.3168× | 0.32× | ✅ |
| static | 25,358 | 41,005 | 0.6184× | 0.62× | ✅ |
| client | 18,596 | 19,730 | 0.9425× | 0.93× | ✅ |
| socket | 19,835 | 21,042 | 0.9426× | 0.93× | ✅ |
| pgsql | 22,888 | 24,663 | 0.9280× | 0.92× | ✅ |
| sqlite | 23,478 | 25,268 | 0.9291× | 0.93× | ✅ |
| curl | 86,077 | 90,546 | 0.9506× | 0.95× | ✅ |
| dns | 230,250 | 233,203 | 0.9873× | 0.99× | ✅ |
| file | 108,880 | 131,211 | 0.8298× | 0.83× | ✅ |

Geometric mean 0.8534×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
