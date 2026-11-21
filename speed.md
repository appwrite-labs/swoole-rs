# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `2acfc42` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,767 | 15,210 | 0.9051× | 0.91× | ✅ |
| echo-1k | 16,806 | 18,162 | 0.9253× | 0.92× | ✅ |
| json | 22,096 | 23,161 | 0.9540× | 0.95× | ✅ |
| service-health | 16,354 | 17,773 | 0.9202× | 0.92× | ✅ |
| websocket | 6,722 | 8,192 | 0.8205× | 0.81× | ✅ |
| timer | 201,930 | 314,010 | 0.6431× | 0.65× | ✅ |
| app | 127,605 | 135,958 | 0.9386× | 0.93× | ✅ |
| http2 | 18,959 | 20,496 | 0.9250× | 0.92× | ✅ |
| table | 194,387 | 202,900 | 0.9580× | 0.96× | ✅ |
| channel | 137,379 | 155,717 | 0.8822× | 0.88× | ✅ |
| task | 32,092 | 33,268 | 0.9647× | 0.96× | ✅ |
| mysql | 50,321 | 51,082 | 0.9851× | 0.97× | ✅ |
| http-client | 33,674 | 35,796 | 0.9407× | 0.93× | ✅ |
| gzip | 194,292 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,570 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,766 | 0.9779× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,986 | 0.9525× | 0.95× | ✅ |
| connect | 17,545 | 18,734 | 0.9366× | 0.93× | ✅ |
| https | 106,323 | 108,802 | 0.9772× | 0.98× | ✅ |
| wss | 101,586 | 104,797 | 0.9694× | 0.97× | ✅ |
| atomic | 50,209 | 51,285 | 0.9790× | 0.97× | ✅ |
| process-mode | 15,435 | 17,024 | 0.9067× | 0.89× | ✅ |
| http2-concurrent | 364,364 | 409,057 | 0.8907× | 0.89× | ✅ |
| websocket-64k | 90,005 | 159,691 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,372 | 0.7283× | 0.72× | ✅ |
| upload | 416,515 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,937 | 41,063 | 0.6316× | 0.64× | ✅ |
| client | 18,631 | 19,730 | 0.9443× | 0.93× | ✅ |
| socket | 19,873 | 21,046 | 0.9443× | 0.93× | ✅ |
| pgsql | 22,830 | 24,662 | 0.9257× | 0.92× | ✅ |
| sqlite | 23,532 | 25,268 | 0.9313× | 0.93× | ✅ |
| curl | 86,139 | 90,416 | 0.9527× | 0.95× | ✅ |
| dns | 229,926 | 232,682 | 0.9882× | 0.99× | ✅ |
| file | 109,096 | 131,297 | 0.8309× | 0.83× | ✅ |

Geometric mean 0.8537×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
