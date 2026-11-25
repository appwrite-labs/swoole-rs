# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `ab5d2a5` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,766 | 15,214 | 0.9048× | 0.91× | ✅ |
| echo-1k | 16,798 | 18,165 | 0.9247× | 0.92× | ✅ |
| json | 22,090 | 23,173 | 0.9533× | 0.95× | ✅ |
| service-health | 16,348 | 17,786 | 0.9191× | 0.92× | ✅ |
| websocket | 6,722 | 8,189 | 0.8209× | 0.81× | ✅ |
| timer | 202,015 | 313,888 | 0.6436× | 0.65× | ✅ |
| app | 127,547 | 135,845 | 0.9389× | 0.93× | ✅ |
| http2 | 19,045 | 20,500 | 0.9290× | 0.92× | ✅ |
| table | 194,320 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,372 | 155,726 | 0.8821× | 0.88× | ✅ |
| task | 32,087 | 33,272 | 0.9644× | 0.96× | ✅ |
| mysql | 50,292 | 51,083 | 0.9845× | 0.97× | ✅ |
| http-client | 33,599 | 35,793 | 0.9387× | 0.93× | ✅ |
| gzip | 194,300 | 197,099 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,191 | 181,570 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,663 | 4,763 | 0.9790× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,228 | 0.4955× | 0.5× | ✅ |
| udp | 7,617 | 7,987 | 0.9538× | 0.95× | ✅ |
| connect | 17,537 | 18,734 | 0.9361× | 0.93× | ✅ |
| https | 106,299 | 108,809 | 0.9769× | 0.98× | ✅ |
| wss | 101,582 | 104,797 | 0.9693× | 0.97× | ✅ |
| atomic | 50,216 | 51,276 | 0.9793× | 0.97× | ✅ |
| process-mode | 15,435 | 17,021 | 0.9068× | 0.89× | ✅ |
| http2-concurrent | 364,397 | 408,864 | 0.8912× | 0.89× | ✅ |
| websocket-64k | 89,999 | 159,695 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,927 | 16,368 | 0.7287× | 0.72× | ✅ |
| upload | 416,512 | 1,322,425 | 0.3150× | 0.32× | ✅ |
| static | 25,943 | 41,092 | 0.6313× | 0.64× | ✅ |
| client | 18,623 | 19,717 | 0.9445× | 0.93× | ✅ |
| socket | 19,866 | 21,036 | 0.9444× | 0.93× | ✅ |
| pgsql | 22,821 | 24,662 | 0.9253× | 0.92× | ✅ |
| sqlite | 23,519 | 25,268 | 0.9308× | 0.93× | ✅ |
| curl | 86,133 | 90,547 | 0.9513× | 0.95× | ✅ |
| dns | 230,361 | 232,716 | 0.9899× | 0.99× | ✅ |
| file | 108,940 | 131,212 | 0.8303× | 0.83× | ✅ |

Geometric mean 0.8538×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
