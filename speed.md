# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `4fde9d6` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,927 | 15,214 | 0.9154× | 0.92× | ✅ |
| echo-1k | 17,071 | 18,165 | 0.9398× | 0.94× | ✅ |
| json | 22,005 | 23,161 | 0.9501× | 0.96× | ✅ |
| service-health | 16,502 | 17,777 | 0.9283× | 0.93× | ✅ |
| websocket | 6,718 | 8,188 | 0.8205× | 0.82× | ✅ |
| timer | 202,492 | 313,598 | 0.6457× | 0.65× | ✅ |
| app | 125,968 | 136,000 | 0.9262× | 0.93× | ✅ |
| http2 | 19,828 | 20,468 | 0.9687× | 0.97× | ✅ |
| table | 195,719 | 201,865 | 0.9696× | 0.97× | ✅ |
| channel | 138,803 | 154,654 | 0.8975× | 0.9× | ✅ |
| task | 31,972 | 33,265 | 0.9611× | 0.96× | ✅ |
| mysql | 49,519 | 51,061 | 0.9698× | 0.97× | ✅ |
| http-client | 33,589 | 35,813 | 0.9379× | 0.94× | ✅ |
| client | 18,459 | 19,757 | 0.9343× | 0.94× | ✅ |
| socket | 19,742 | 21,067 | 0.9371× | 0.92× | ✅ |
| gzip | 196,006 | 197,079 | 0.9946× | 1.0× | ✅ |
| websocket-deflate | 181,473 | 181,568 | 0.9995× | 1.0× | ✅ |
| tcp-length | 5,022 | 4,762 | 1.0545× | 1.05× | ❌ |
| tcp-eof | 5,564 | 10,228 | 0.5440× | 0.54× | ✅ |
| udp | 8,033 | 7,987 | 1.0058× | 1.0× | ❌ |
| connect | 18,254 | 18,707 | 0.9758× | 0.98× | ✅ |
| https | 108,817 | 108,813 | 1.0000× | 1.01× | ➖ |
| wss | 104,664 | 104,794 | 0.9988× | 1.0× | ✅ |
| atomic | 49,670 | 51,316 | 0.9679× | 0.97× | ✅ |
| process-mode | 16,420 | 16,956 | 0.9684× | 0.97× | ✅ |
| http2-concurrent | 372,171 | 408,877 | 0.9102× | 0.92× | ✅ |
| websocket-64k | 156,165 | 159,700 | 0.9779× | 0.98× | ✅ |
| websocket-fragmented | 13,076 | 16,331 | 0.8007× | 0.79× | ✅ |
| upload | 544,637 | 1,322,380 | 0.4119× | 0.42× | ✅ |
| static | 34,502 | 41,103 | 0.8394× | 0.84× | ✅ |
| pgsql | 23,209 | 24,594 | 0.9437× | 0.95× | ✅ |
| sqlite | 23,487 | 25,232 | 0.9309× | 0.94× | ✅ |
| curl | 85,222 | 90,380 | 0.9429× | 0.95× | ✅ |
| dns | 231,048 | 232,674 | 0.9930× | 1.0× | ✅ |

Geometric mean 0.8977×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
