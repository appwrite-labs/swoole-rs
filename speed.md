# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `206eb99` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,941 | 15,214 | 0.9163× | 0.92× | ✅ |
| echo-1k | 17,044 | 18,165 | 0.9383× | 0.94× | ✅ |
| json | 22,024 | 23,161 | 0.9509× | 0.95× | ✅ |
| service-health | 16,522 | 17,777 | 0.9294× | 0.93× | ✅ |
| websocket | 6,586 | 8,188 | 0.8043× | 0.82× | ✅ |
| timer | 203,397 | 313,615 | 0.6486× | 0.65× | ✅ |
| app | 125,670 | 135,973 | 0.9242× | 0.93× | ✅ |
| http2 | 18,650 | 20,468 | 0.9112× | 0.93× | ✅ |
| table | 195,475 | 201,865 | 0.9683× | 0.97× | ✅ |
| channel | 138,446 | 154,654 | 0.8952× | 0.9× | ✅ |
| task | 32,121 | 33,294 | 0.9648× | 0.96× | ✅ |
| mysql | 49,524 | 51,006 | 0.9709× | 0.97× | ✅ |
| http-client | 33,709 | 35,805 | 0.9414× | 0.94× | ✅ |
| client | 18,444 | 19,757 | 0.9335× | 0.94× | ✅ |
| socket | 19,588 | 21,067 | 0.9298× | 0.92× | ✅ |
| gzip | 194,143 | 197,079 | 0.9851× | 1.0× | ✅ |
| websocket-deflate | 179,402 | 181,572 | 0.9880× | 1.0× | ✅ |
| tcp-length | 4,797 | 4,762 | 1.0073× | 1.05× | ➖ |
| tcp-eof | 5,345 | 10,228 | 0.5225× | 0.54× | ✅ |
| udp | 7,572 | 7,987 | 0.9480× | 0.95× | ✅ |
| connect | 18,363 | 18,707 | 0.9816× | 0.98× | ✅ |
| https | 108,800 | 108,810 | 0.9999× | 1.0× | ✅ |
| wss | 104,482 | 104,794 | 0.9970× | 1.0× | ✅ |
| atomic | 49,682 | 51,316 | 0.9682× | 0.97× | ✅ |
| process-mode | 16,703 | 16,963 | 0.9847× | 0.99× | ✅ |
| http2-concurrent | 359,224 | 408,877 | 0.8786× | 0.88× | ✅ |
| websocket-64k | 156,031 | 159,691 | 0.9771× | 0.98× | ✅ |
| websocket-fragmented | 12,830 | 16,331 | 0.7857× | 0.79× | ✅ |
| upload | 478,026 | 1,322,382 | 0.3615× | 0.37× | ✅ |
| static | 29,292 | 41,074 | 0.7131× | 0.72× | ✅ |
| pgsql | 23,211 | 24,595 | 0.9437× | 0.95× | ✅ |
| sqlite | 23,492 | 25,236 | 0.9309× | 0.94× | ✅ |
| curl | 84,855 | 90,479 | 0.9378× | 0.94× | ✅ |
| dns | 231,106 | 232,483 | 0.9941× | 0.99× | ✅ |

Geometric mean 0.8825×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
