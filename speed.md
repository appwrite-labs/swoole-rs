# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `4d0a3dc` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,755 | 15,213 | 0.9041× | 0.91× | ✅ |
| echo-1k | 16,797 | 18,165 | 0.9247× | 0.92× | ✅ |
| json | 22,080 | 23,164 | 0.9532× | 0.95× | ✅ |
| service-health | 16,338 | 17,777 | 0.9190× | 0.92× | ✅ |
| websocket | 6,722 | 8,189 | 0.8209× | 0.81× | ✅ |
| timer | 201,935 | 313,854 | 0.6434× | 0.65× | ✅ |
| app | 127,625 | 135,903 | 0.9391× | 0.93× | ✅ |
| http2 | 18,970 | 20,500 | 0.9254× | 0.92× | ✅ |
| table | 194,374 | 202,900 | 0.9580× | 0.96× | ✅ |
| channel | 137,362 | 155,717 | 0.8821× | 0.88× | ✅ |
| task | 32,087 | 33,269 | 0.9645× | 0.96× | ✅ |
| mysql | 50,308 | 51,085 | 0.9848× | 0.97× | ✅ |
| http-client | 33,669 | 35,787 | 0.9408× | 0.93× | ✅ |
| gzip | 194,275 | 197,095 | 0.9857× | 0.99× | ✅ |
| websocket-deflate | 179,190 | 181,565 | 0.9869× | 0.99× | ✅ |
| tcp-length | 4,661 | 4,766 | 0.9779× | 0.96× | ✅ |
| tcp-eof | 5,068 | 10,231 | 0.4953× | 0.5× | ✅ |
| udp | 7,607 | 7,986 | 0.9526× | 0.95× | ✅ |
| connect | 17,529 | 18,734 | 0.9357× | 0.93× | ✅ |
| https | 106,310 | 108,809 | 0.9770× | 0.98× | ✅ |
| wss | 101,589 | 104,797 | 0.9694× | 0.97× | ✅ |
| atomic | 50,193 | 51,276 | 0.9789× | 0.97× | ✅ |
| process-mode | 15,412 | 17,025 | 0.9053× | 0.89× | ✅ |
| http2-concurrent | 364,596 | 408,864 | 0.8917× | 0.89× | ✅ |
| websocket-64k | 90,002 | 159,694 | 0.5636× | 0.57× | ✅ |
| websocket-fragmented | 11,923 | 16,372 | 0.7283× | 0.72× | ✅ |
| upload | 416,524 | 1,322,429 | 0.3150× | 0.32× | ✅ |
| static | 25,848 | 40,999 | 0.6304× | 0.64× | ✅ |
| client | 18,621 | 19,721 | 0.9442× | 0.93× | ✅ |
| socket | 19,860 | 21,036 | 0.9441× | 0.93× | ✅ |
| pgsql | 22,826 | 24,663 | 0.9255× | 0.92× | ✅ |
| sqlite | 23,527 | 25,268 | 0.9311× | 0.93× | ✅ |
| curl | 86,131 | 90,419 | 0.9526× | 0.95× | ✅ |
| dns | 229,916 | 232,923 | 0.9871× | 0.99× | ✅ |
| file | 109,096 | 131,297 | 0.8309× | 0.83× | ✅ |

Geometric mean 0.8535×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
