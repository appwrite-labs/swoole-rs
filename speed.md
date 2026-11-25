# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `ab5d2a5` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,770 | 15,214 | 0.9051× | 0.91× | ✅ |
| echo-1k | 16,798 | 18,165 | 0.9247× | 0.92× | ✅ |
| json | 22,090 | 23,173 | 0.9532× | 0.95× | ✅ |
| service-health | 16,348 | 17,786 | 0.9191× | 0.92× | ✅ |
| websocket | 6,722 | 8,189 | 0.8209× | 0.81× | ✅ |
| timer | 201,970 | 313,658 | 0.6439× | 0.65× | ✅ |
| app | 127,545 | 135,938 | 0.9383× | 0.93× | ✅ |
| http2 | 19,044 | 20,499 | 0.9290× | 0.92× | ✅ |
| table | 194,320 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,372 | 155,723 | 0.8822× | 0.88× | ✅ |
| task | 32,090 | 33,268 | 0.9646× | 0.96× | ✅ |
| mysql | 50,296 | 51,085 | 0.9846× | 0.97× | ✅ |
| http-client | 33,597 | 35,791 | 0.9387× | 0.93× | ✅ |
| gzip | 194,300 | 197,099 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,194 | 181,573 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,663 | 4,763 | 0.9790× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,617 | 7,986 | 0.9537× | 0.95× | ✅ |
| connect | 17,540 | 18,730 | 0.9365× | 0.93× | ✅ |
| https | 106,303 | 108,806 | 0.9770× | 0.98× | ✅ |
| wss | 101,582 | 104,797 | 0.9693× | 0.97× | ✅ |
| atomic | 50,206 | 51,285 | 0.9790× | 0.97× | ✅ |
| process-mode | 15,432 | 17,027 | 0.9063× | 0.89× | ✅ |
| http2-concurrent | 364,189 | 409,056 | 0.8903× | 0.89× | ✅ |
| websocket-64k | 90,000 | 159,694 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,928 | 16,368 | 0.7287× | 0.72× | ✅ |
| upload | 416,510 | 1,322,425 | 0.3150× | 0.32× | ✅ |
| static | 25,914 | 41,063 | 0.6311× | 0.64× | ✅ |
| client | 18,610 | 19,727 | 0.9434× | 0.93× | ✅ |
| socket | 19,856 | 21,045 | 0.9435× | 0.93× | ✅ |
| pgsql | 22,820 | 24,663 | 0.9253× | 0.92× | ✅ |
| sqlite | 23,515 | 25,268 | 0.9306× | 0.93× | ✅ |
| curl | 86,142 | 90,538 | 0.9514× | 0.95× | ✅ |
| dns | 230,126 | 233,081 | 0.9873× | 0.99× | ✅ |
| file | 108,940 | 131,211 | 0.8303× | 0.83× | ✅ |

Geometric mean 0.8537×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
