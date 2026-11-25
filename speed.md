# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `ab5d2a5` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,766 | 15,214 | 0.9048× | 0.91× | ✅ |
| echo-1k | 16,798 | 18,165 | 0.9247× | 0.92× | ✅ |
| json | 22,103 | 23,164 | 0.9542× | 0.95× | ✅ |
| service-health | 16,361 | 17,777 | 0.9204× | 0.92× | ✅ |
| websocket | 6,726 | 8,189 | 0.8213× | 0.81× | ✅ |
| timer | 202,004 | 313,859 | 0.6436× | 0.65× | ✅ |
| app | 127,527 | 135,942 | 0.9381× | 0.93× | ✅ |
| http2 | 19,053 | 20,501 | 0.9294× | 0.92× | ✅ |
| table | 194,394 | 202,900 | 0.9581× | 0.96× | ✅ |
| channel | 137,382 | 155,717 | 0.8823× | 0.88× | ✅ |
| task | 32,090 | 33,265 | 0.9647× | 0.96× | ✅ |
| mysql | 50,275 | 51,085 | 0.9841× | 0.97× | ✅ |
| http-client | 33,620 | 35,788 | 0.9394× | 0.93× | ✅ |
| gzip | 194,284 | 197,095 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,191 | 181,565 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,663 | 4,763 | 0.9790× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,228 | 0.4955× | 0.5× | ✅ |
| udp | 7,617 | 7,987 | 0.9538× | 0.95× | ✅ |
| connect | 17,540 | 18,730 | 0.9365× | 0.93× | ✅ |
| https | 106,303 | 108,813 | 0.9769× | 0.98× | ✅ |
| wss | 101,579 | 104,800 | 0.9693× | 0.97× | ✅ |
| atomic | 50,203 | 51,285 | 0.9789× | 0.97× | ✅ |
| process-mode | 15,432 | 17,027 | 0.9063× | 0.89× | ✅ |
| http2-concurrent | 364,425 | 409,057 | 0.8909× | 0.89× | ✅ |
| websocket-64k | 89,999 | 159,695 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,928 | 16,368 | 0.7287× | 0.72× | ✅ |
| upload | 416,510 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,914 | 41,063 | 0.6311× | 0.64× | ✅ |
| client | 18,613 | 19,729 | 0.9434× | 0.93× | ✅ |
| socket | 19,853 | 21,045 | 0.9434× | 0.93× | ✅ |
| pgsql | 22,821 | 24,663 | 0.9253× | 0.92× | ✅ |
| sqlite | 23,519 | 25,268 | 0.9308× | 0.93× | ✅ |
| curl | 86,132 | 90,420 | 0.9526× | 0.95× | ✅ |
| dns | 230,035 | 232,902 | 0.9877× | 0.99× | ✅ |
| file | 108,936 | 131,300 | 0.8297× | 0.83× | ✅ |

Geometric mean 0.8536×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
