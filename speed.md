# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `ab5d2a5` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,770 | 15,213 | 0.9051× | 0.91× | ✅ |
| echo-1k | 16,798 | 18,165 | 0.9247× | 0.92× | ✅ |
| json | 22,090 | 23,173 | 0.9532× | 0.95× | ✅ |
| service-health | 16,348 | 17,786 | 0.9191× | 0.92× | ✅ |
| websocket | 6,722 | 8,189 | 0.8209× | 0.81× | ✅ |
| timer | 201,952 | 313,786 | 0.6436× | 0.65× | ✅ |
| app | 127,537 | 135,948 | 0.9381× | 0.93× | ✅ |
| http2 | 19,030 | 20,500 | 0.9283× | 0.92× | ✅ |
| table | 194,316 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,372 | 155,726 | 0.8821× | 0.88× | ✅ |
| task | 32,091 | 33,268 | 0.9646× | 0.96× | ✅ |
| mysql | 50,275 | 51,085 | 0.9841× | 0.97× | ✅ |
| http-client | 33,618 | 35,787 | 0.9394× | 0.93× | ✅ |
| gzip | 194,284 | 197,095 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,191 | 181,562 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,663 | 4,764 | 0.9788× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,618 | 7,987 | 0.9538× | 0.95× | ✅ |
| connect | 17,540 | 18,730 | 0.9365× | 0.93× | ✅ |
| https | 106,303 | 108,802 | 0.9770× | 0.98× | ✅ |
| wss | 101,582 | 104,797 | 0.9693× | 0.97× | ✅ |
| atomic | 50,206 | 51,285 | 0.9790× | 0.97× | ✅ |
| process-mode | 15,434 | 17,024 | 0.9066× | 0.89× | ✅ |
| http2-concurrent | 364,273 | 409,058 | 0.8905× | 0.89× | ✅ |
| websocket-64k | 90,003 | 159,694 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,924 | 16,372 | 0.7283× | 0.72× | ✅ |
| upload | 416,506 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,914 | 41,063 | 0.6311× | 0.64× | ✅ |
| client | 18,613 | 19,730 | 0.9434× | 0.93× | ✅ |
| socket | 19,856 | 21,046 | 0.9435× | 0.93× | ✅ |
| pgsql | 22,821 | 24,662 | 0.9253× | 0.92× | ✅ |
| sqlite | 23,519 | 25,268 | 0.9308× | 0.93× | ✅ |
| curl | 86,132 | 90,550 | 0.9512× | 0.95× | ✅ |
| dns | 229,734 | 233,208 | 0.9851× | 0.99× | ✅ |
| file | 108,936 | 131,300 | 0.8297× | 0.83× | ✅ |

Geometric mean 0.8536×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
