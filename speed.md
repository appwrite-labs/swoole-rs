# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `d8c1f0c` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,937 | 15,213 | 0.9161× | 0.92× | ✅ |
| echo-1k | 17,044 | 18,166 | 0.9382× | 0.94× | ✅ |
| json | 22,027 | 23,158 | 0.9512× | 0.95× | ✅ |
| service-health | 16,526 | 17,777 | 0.9296× | 0.93× | ✅ |
| websocket | 6,589 | 8,188 | 0.8048× | 0.82× | ✅ |
| timer | 203,449 | 313,506 | 0.6489× | 0.65× | ✅ |
| app | 125,680 | 135,932 | 0.9246× | 0.93× | ✅ |
| http2 | 18,656 | 20,464 | 0.9116× | 0.93× | ✅ |
| table | 195,479 | 201,865 | 0.9684× | 0.97× | ✅ |
| channel | 138,443 | 154,654 | 0.8952× | 0.9× | ✅ |
| task | 32,122 | 33,268 | 0.9656× | 0.96× | ✅ |
| mysql | 49,515 | 51,064 | 0.9697× | 0.97× | ✅ |
| http-client | 33,707 | 35,804 | 0.9414× | 0.94× | ✅ |
| client | 18,448 | 19,757 | 0.9338× | 0.94× | ✅ |
| socket | 19,588 | 21,067 | 0.9298× | 0.92× | ✅ |
| gzip | 194,146 | 197,083 | 0.9851× | 1.0× | ✅ |
| websocket-deflate | 179,402 | 181,572 | 0.9880× | 1.0× | ✅ |
| tcp-length | 4,797 | 4,762 | 1.0073× | 1.05× | ➖ |
| tcp-eof | 5,345 | 10,228 | 0.5225× | 0.54× | ✅ |
| udp | 7,572 | 7,986 | 0.9481× | 0.95× | ✅ |
| connect | 18,363 | 18,707 | 0.9816× | 0.98× | ✅ |
| https | 108,785 | 108,810 | 0.9998× | 1.0× | ✅ |
| wss | 104,482 | 104,794 | 0.9970× | 1.0× | ✅ |
| atomic | 49,682 | 51,316 | 0.9682× | 0.97× | ✅ |
| process-mode | 16,703 | 17,027 | 0.9810× | 0.99× | ✅ |
| http2-concurrent | 359,138 | 408,877 | 0.8784× | 0.88× | ✅ |
| websocket-64k | 156,027 | 159,692 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 12,830 | 16,331 | 0.7857× | 0.79× | ✅ |
| upload | 478,037 | 1,322,382 | 0.3615× | 0.37× | ✅ |
| static | 29,309 | 41,074 | 0.7136× | 0.72× | ✅ |
| pgsql | 23,209 | 24,594 | 0.9437× | 0.95× | ✅ |
| sqlite | 23,488 | 25,232 | 0.9309× | 0.94× | ✅ |
| curl | 84,851 | 90,349 | 0.9391× | 0.94× | ✅ |
| dns | 230,637 | 233,364 | 0.9883× | 0.99× | ✅ |

Geometric mean 0.8824×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
