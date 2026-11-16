# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `0c38824` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,917 | 15,214 | 0.9147× | 0.91× | ✅ |
| echo-1k | 16,942 | 18,165 | 0.9326× | 0.93× | ✅ |
| json | 22,054 | 23,161 | 0.9522× | 0.95× | ✅ |
| service-health | 16,501 | 17,777 | 0.9282× | 0.93× | ✅ |
| websocket | 6,564 | 8,191 | 0.8013× | 0.8× | ✅ |
| timer | 206,534 | 313,765 | 0.6582× | 0.65× | ✅ |
| app | 126,363 | 135,939 | 0.9296× | 0.93× | ✅ |
| http2 | 19,065 | 20,464 | 0.9316× | 0.92× | ✅ |
| table | 195,166 | 201,869 | 0.9668× | 0.97× | ✅ |
| channel | 137,545 | 155,170 | 0.8864× | 0.88× | ✅ |
| task | 32,472 | 33,261 | 0.9763× | 0.96× | ✅ |
| mysql | 49,726 | 51,061 | 0.9739× | 0.97× | ✅ |
| http-client | 33,562 | 35,799 | 0.9375× | 0.93× | ✅ |
| gzip | 194,019 | 197,079 | 0.9845× | 0.99× | ✅ |
| websocket-deflate | 179,017 | 181,568 | 0.9859× | 0.99× | ✅ |
| tcp-length | 4,638 | 4,765 | 0.9733× | 0.98× | ✅ |
| tcp-eof | 5,198 | 10,231 | 0.5081× | 0.51× | ✅ |
| udp | 7,537 | 7,987 | 0.9437× | 0.93× | ✅ |
| connect | 17,840 | 18,707 | 0.9537× | 0.94× | ✅ |
| https | 106,032 | 108,810 | 0.9745× | 1.0× | ✅ |
| wss | 100,896 | 104,801 | 0.9627× | 0.99× | ✅ |
| atomic | 50,020 | 51,316 | 0.9748× | 0.97× | ✅ |
| process-mode | 15,098 | 16,960 | 0.8902× | 0.88× | ✅ |
| http2-concurrent | 361,885 | 409,072 | 0.8846× | 0.88× | ✅ |
| websocket-64k | 89,866 | 159,693 | 0.5627× | 0.57× | ✅ |
| websocket-fragmented | 11,761 | 16,327 | 0.7203× | 0.72× | ✅ |
| upload | 414,002 | 1,322,389 | 0.3131× | 0.32× | ✅ |
| static | 27,044 | 40,993 | 0.6597× | 0.66× | ✅ |
| client | 18,599 | 19,756 | 0.9414× | 0.94× | ✅ |
| socket | 19,753 | 21,073 | 0.9374× | 0.92× | ✅ |
| pgsql | 23,491 | 24,594 | 0.9551× | 0.95× | ✅ |
| sqlite | 23,608 | 25,236 | 0.9355× | 0.93× | ✅ |
| curl | 86,009 | 90,482 | 0.9506× | 0.94× | ✅ |
| dns | 230,342 | 233,169 | 0.9879× | 0.99× | ✅ |

Geometric mean 0.8561×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
