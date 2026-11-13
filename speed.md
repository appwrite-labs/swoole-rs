# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `14b4d5d` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,941 | 15,213 | 0.9164× | 0.92× | ✅ |
| echo-1k | 17,044 | 18,165 | 0.9383× | 0.94× | ✅ |
| json | 22,027 | 23,161 | 0.9510× | 0.95× | ✅ |
| service-health | 16,526 | 17,773 | 0.9298× | 0.93× | ✅ |
| websocket | 6,585 | 8,191 | 0.8039× | 0.82× | ✅ |
| timer | 203,452 | 313,605 | 0.6488× | 0.65× | ✅ |
| app | 125,667 | 135,855 | 0.9250× | 0.93× | ✅ |
| http2 | 18,651 | 20,467 | 0.9113× | 0.93× | ✅ |
| table | 195,475 | 201,865 | 0.9683× | 0.97× | ✅ |
| channel | 138,443 | 154,654 | 0.8952× | 0.9× | ✅ |
| task | 32,133 | 33,268 | 0.9659× | 0.96× | ✅ |
| mysql | 49,520 | 51,058 | 0.9699× | 0.97× | ✅ |
| http-client | 33,711 | 35,805 | 0.9415× | 0.94× | ✅ |
| client | 18,448 | 19,756 | 0.9338× | 0.94× | ✅ |
| socket | 19,588 | 21,067 | 0.9298× | 0.92× | ✅ |
| gzip | 194,146 | 197,079 | 0.9851× | 1.0× | ✅ |
| websocket-deflate | 179,402 | 181,572 | 0.9880× | 1.0× | ✅ |
| tcp-length | 4,797 | 4,762 | 1.0073× | 1.05× | ➖ |
| tcp-eof | 5,345 | 10,228 | 0.5225× | 0.54× | ✅ |
| udp | 7,572 | 7,987 | 0.9481× | 0.95× | ✅ |
| connect | 18,363 | 18,703 | 0.9818× | 0.98× | ✅ |
| https | 108,786 | 108,813 | 0.9997× | 1.0× | ✅ |
| wss | 104,478 | 104,801 | 0.9969× | 1.0× | ✅ |
| atomic | 49,686 | 51,316 | 0.9682× | 0.97× | ✅ |
| process-mode | 16,702 | 16,959 | 0.9848× | 0.99× | ✅ |
| http2-concurrent | 358,972 | 408,882 | 0.8779× | 0.88× | ✅ |
| websocket-64k | 156,033 | 159,699 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 13,040 | 16,327 | 0.7987× | 0.79× | ✅ |
| upload | 478,085 | 1,322,382 | 0.3615× | 0.37× | ✅ |
| static | 29,321 | 41,074 | 0.7139× | 0.72× | ✅ |
| pgsql | 23,207 | 24,595 | 0.9435× | 0.95× | ✅ |
| sqlite | 23,492 | 25,236 | 0.9309× | 0.94× | ✅ |
| curl | 84,855 | 90,351 | 0.9392× | 0.94× | ✅ |
| dns | 230,647 | 233,258 | 0.9888× | 0.99× | ✅ |

Geometric mean 0.8829×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
