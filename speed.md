# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `b4100dc` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,940 | 15,213 | 0.9163× | 0.92× | ✅ |
| echo-1k | 17,044 | 18,165 | 0.9383× | 0.94× | ✅ |
| json | 22,024 | 23,161 | 0.9509× | 0.95× | ✅ |
| service-health | 16,526 | 17,777 | 0.9296× | 0.93× | ✅ |
| websocket | 6,586 | 8,191 | 0.8040× | 0.82× | ✅ |
| timer | 203,528 | 313,533 | 0.6491× | 0.65× | ✅ |
| app | 125,697 | 135,860 | 0.9252× | 0.93× | ✅ |
| http2 | 18,650 | 20,468 | 0.9112× | 0.93× | ✅ |
| table | 195,475 | 201,865 | 0.9683× | 0.97× | ✅ |
| channel | 138,446 | 154,654 | 0.8952× | 0.9× | ✅ |
| task | 32,125 | 33,261 | 0.9658× | 0.96× | ✅ |
| mysql | 49,526 | 51,064 | 0.9699× | 0.97× | ✅ |
| http-client | 33,710 | 35,809 | 0.9414× | 0.94× | ✅ |
| client | 18,448 | 19,758 | 0.9337× | 0.94× | ✅ |
| socket | 19,588 | 21,067 | 0.9298× | 0.92× | ✅ |
| gzip | 194,143 | 197,079 | 0.9851× | 1.0× | ✅ |
| websocket-deflate | 179,402 | 181,568 | 0.9881× | 1.0× | ✅ |
| tcp-length | 4,797 | 4,762 | 1.0073× | 1.05× | ➖ |
| tcp-eof | 5,345 | 10,231 | 0.5224× | 0.54× | ✅ |
| udp | 7,572 | 7,987 | 0.9480× | 0.95× | ✅ |
| connect | 18,364 | 18,703 | 0.9818× | 0.98× | ✅ |
| https | 108,804 | 108,810 | 0.9999× | 1.0× | ✅ |
| wss | 104,482 | 104,797 | 0.9970× | 1.0× | ✅ |
| atomic | 49,686 | 51,316 | 0.9682× | 0.97× | ✅ |
| process-mode | 16,703 | 16,959 | 0.9849× | 0.99× | ✅ |
| http2-concurrent | 359,174 | 408,877 | 0.8784× | 0.88× | ✅ |
| websocket-64k | 156,029 | 159,694 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 13,037 | 16,331 | 0.7983× | 0.79× | ✅ |
| upload | 478,085 | 1,322,382 | 0.3615× | 0.37× | ✅ |
| static | 29,321 | 41,074 | 0.7139× | 0.72× | ✅ |
| pgsql | 23,211 | 24,595 | 0.9437× | 0.95× | ✅ |
| sqlite | 23,492 | 25,236 | 0.9309× | 0.94× | ✅ |
| curl | 84,852 | 90,320 | 0.9395× | 0.94× | ✅ |
| dns | 230,741 | 232,766 | 0.9913× | 0.99× | ✅ |

Geometric mean 0.8830×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
