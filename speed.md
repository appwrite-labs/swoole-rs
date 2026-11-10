# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `cdfc370` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,385 | 15,205 | 0.9461× | 0.91× | ✅ |
| echo-1k | 17,368 | 18,172 | 0.9558× | 0.92× | ✅ |
| json | 22,409 | 23,176 | 0.9669× | 0.94× | ✅ |
| service-health | 16,980 | 17,783 | 0.9549× | 0.93× | ✅ |
| websocket | 6,565 | 8,188 | 0.8018× | 0.81× | ✅ |
| timer | 201,544 | 313,382 | 0.6431× | 0.67× | ✅ |
| app | 125,639 | 136,021 | 0.9237× | 0.92× | ✅ |
| http2 | 19,789 | 20,485 | 0.9660× | 0.95× | ✅ |
| table | 197,938 | 201,934 | 0.9802× | 0.98× | ✅ |
| channel | 139,798 | 155,262 | 0.9004× | 0.89× | ✅ |
| task | 31,901 | 33,311 | 0.9577× | 0.94× | ✅ |
| mysql | 49,416 | 51,065 | 0.9677× | 0.97× | ✅ |
| http-client | 33,593 | 35,828 | 0.9376× | 0.95× | ✅ |
| client | 18,443 | 18,938 | 0.9739× | 0.95× | ✅ |
| socket | 17,803 | 19,124 | 0.9309× | 0.99× | ✅ |
| gzip | 200,800 | 197,095 | 1.0188× | 1.02× | ➖ |
| websocket-deflate | 181,365 | 181,568 | 0.9989× | 1.0× | ✅ |
| tcp-length | 5,054 | 4,744 | 1.0655× | 1.02× | ❌ |
| tcp-eof | 5,592 | 10,209 | 0.5478× | 0.55× | ✅ |
| udp | 7,831 | 7,332 | 1.0681× | 1.09× | ➖ |
| connect | 20,303 | 18,707 | 1.0853× | 1.04× | ❌ |
| https | 109,160 | 108,803 | 1.0033× | 1.04× | ➖ |
| wss | 104,245 | 104,811 | 0.9946× | 1.04× | ✅ |
| atomic | 52,370 | 51,334 | 1.0202× | 1.02× | ❌ |
| process-mode | 16,518 | 16,952 | 0.9744× | 0.95× | ✅ |
| http2-concurrent | 1,469,669 | 409,100 | 3.5924× | 1.31× | ❌ |
| websocket-64k | 156,013 | 159,692 | 0.9770× | 0.62× | ✅ |
| websocket-fragmented | 13,547 | 16,193 | 0.8366× | 0.78× | ✅ |
| upload | 1,780,593 | 1,322,473 | 1.3464× | 1.05× | ❌ |
| static | 39,212 | 41,010 | 0.9561× | 1.03× | ✅ |

Geometric mean 0.9877×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
