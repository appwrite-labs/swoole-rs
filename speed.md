# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `09810e9` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,941 | 15,214 | 0.9163× | 0.92× | ✅ |
| echo-1k | 17,047 | 18,165 | 0.9385× | 0.94× | ✅ |
| json | 22,027 | 23,162 | 0.9510× | 0.95× | ✅ |
| service-health | 16,522 | 17,777 | 0.9294× | 0.93× | ✅ |
| websocket | 6,586 | 8,188 | 0.8043× | 0.82× | ✅ |
| timer | 203,430 | 313,565 | 0.6488× | 0.65× | ✅ |
| app | 125,720 | 135,935 | 0.9249× | 0.93× | ✅ |
| http2 | 18,649 | 20,468 | 0.9111× | 0.93× | ✅ |
| table | 195,475 | 201,865 | 0.9683× | 0.97× | ✅ |
| channel | 138,443 | 154,654 | 0.8952× | 0.9× | ✅ |
| task | 32,124 | 33,268 | 0.9656× | 0.96× | ✅ |
| mysql | 49,511 | 51,062 | 0.9696× | 0.97× | ✅ |
| http-client | 33,710 | 35,805 | 0.9415× | 0.94× | ✅ |
| client | 18,448 | 19,757 | 0.9338× | 0.94× | ✅ |
| socket | 19,588 | 21,067 | 0.9298× | 0.92× | ✅ |
| gzip | 194,143 | 197,079 | 0.9851× | 1.0× | ✅ |
| websocket-deflate | 179,402 | 181,568 | 0.9881× | 1.0× | ✅ |
| tcp-length | 4,797 | 4,762 | 1.0073× | 1.05× | ➖ |
| tcp-eof | 5,345 | 10,231 | 0.5224× | 0.54× | ✅ |
| udp | 7,572 | 7,987 | 0.9480× | 0.95× | ✅ |
| connect | 18,363 | 18,707 | 0.9816× | 0.98× | ✅ |
| https | 108,804 | 108,810 | 0.9999× | 1.0× | ✅ |
| wss | 104,496 | 104,794 | 0.9972× | 1.0× | ✅ |
| atomic | 49,682 | 51,316 | 0.9682× | 0.97× | ✅ |
| process-mode | 16,703 | 16,960 | 0.9849× | 0.99× | ✅ |
| http2-concurrent | 359,323 | 408,877 | 0.8788× | 0.88× | ✅ |
| websocket-64k | 156,035 | 159,696 | 0.9771× | 0.98× | ✅ |
| websocket-fragmented | 12,830 | 16,331 | 0.7857× | 0.79× | ✅ |
| upload | 478,027 | 1,322,383 | 0.3615× | 0.37× | ✅ |
| static | 29,292 | 41,045 | 0.7136× | 0.72× | ✅ |
| pgsql | 23,210 | 24,595 | 0.9437× | 0.95× | ✅ |
| sqlite | 23,492 | 25,236 | 0.9309× | 0.94× | ✅ |
| curl | 84,852 | 90,354 | 0.9391× | 0.94× | ✅ |
| dns | 230,638 | 233,150 | 0.9892× | 0.99× | ✅ |

Geometric mean 0.8825×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
