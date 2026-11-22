# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `ea17ff0` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,755 | 15,214 | 0.9041× | 0.91× | ✅ |
| echo-1k | 16,797 | 18,165 | 0.9247× | 0.92× | ✅ |
| json | 22,083 | 23,173 | 0.9530× | 0.95× | ✅ |
| service-health | 16,338 | 17,786 | 0.9186× | 0.92× | ✅ |
| websocket | 6,722 | 8,189 | 0.8209× | 0.81× | ✅ |
| timer | 201,984 | 313,783 | 0.6437× | 0.65× | ✅ |
| app | 127,654 | 135,975 | 0.9388× | 0.93× | ✅ |
| http2 | 18,964 | 20,496 | 0.9253× | 0.92× | ✅ |
| table | 194,310 | 201,885 | 0.9625× | 0.96× | ✅ |
| channel | 137,362 | 155,723 | 0.8821× | 0.88× | ✅ |
| task | 32,084 | 33,272 | 0.9643× | 0.96× | ✅ |
| mysql | 50,311 | 51,088 | 0.9848× | 0.97× | ✅ |
| http-client | 33,625 | 35,792 | 0.9395× | 0.93× | ✅ |
| gzip | 194,287 | 197,095 | 0.9858× | 0.99× | ✅ |
| websocket-deflate | 179,193 | 181,573 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,763 | 0.9786× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,987 | 0.9525× | 0.95× | ✅ |
| connect | 17,532 | 18,734 | 0.9359× | 0.93× | ✅ |
| https | 106,309 | 108,802 | 0.9771× | 0.98× | ✅ |
| wss | 101,595 | 104,797 | 0.9694× | 0.97× | ✅ |
| atomic | 50,196 | 51,285 | 0.9788× | 0.97× | ✅ |
| process-mode | 15,412 | 17,027 | 0.9051× | 0.89× | ✅ |
| http2-concurrent | 364,171 | 409,057 | 0.8903× | 0.89× | ✅ |
| websocket-64k | 89,998 | 159,694 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,927 | 16,368 | 0.7287× | 0.72× | ✅ |
| upload | 416,513 | 1,322,425 | 0.3150× | 0.32× | ✅ |
| static | 25,895 | 41,034 | 0.6311× | 0.64× | ✅ |
| client | 18,623 | 19,730 | 0.9439× | 0.93× | ✅ |
| socket | 19,857 | 21,046 | 0.9435× | 0.93× | ✅ |
| pgsql | 22,822 | 24,662 | 0.9254× | 0.92× | ✅ |
| sqlite | 23,527 | 25,268 | 0.9311× | 0.93× | ✅ |
| curl | 86,134 | 90,418 | 0.9526× | 0.95× | ✅ |
| dns | 229,586 | 232,633 | 0.9869× | 0.99× | ✅ |
| file | 109,096 | 131,207 | 0.8315× | 0.83× | ✅ |

Geometric mean 0.8535×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
