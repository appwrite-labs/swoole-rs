# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `c906698` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,943 | 15,214 | 0.9165× | 0.92× | ✅ |
| echo-1k | 17,041 | 18,166 | 0.9381× | 0.94× | ✅ |
| json | 22,023 | 23,161 | 0.9508× | 0.95× | ✅ |
| service-health | 16,524 | 17,777 | 0.9295× | 0.93× | ✅ |
| websocket | 6,581 | 8,191 | 0.8034× | 0.82× | ✅ |
| timer | 203,465 | 313,766 | 0.6485× | 0.65× | ✅ |
| app | 125,428 | 135,929 | 0.9227× | 0.93× | ✅ |
| http2 | 18,640 | 20,464 | 0.9109× | 0.93× | ✅ |
| table | 195,481 | 201,869 | 0.9684× | 0.97× | ✅ |
| channel | 138,454 | 154,651 | 0.8953× | 0.9× | ✅ |
| task | 31,990 | 33,265 | 0.9617× | 0.96× | ✅ |
| mysql | 49,494 | 51,064 | 0.9692× | 0.97× | ✅ |
| http-client | 33,663 | 35,803 | 0.9402× | 0.94× | ✅ |
| client | 18,435 | 19,753 | 0.9333× | 0.94× | ✅ |
| socket | 19,588 | 21,067 | 0.9298× | 0.92× | ✅ |
| gzip | 194,148 | 197,079 | 0.9851× | 1.0× | ✅ |
| websocket-deflate | 179,401 | 181,572 | 0.9880× | 1.0× | ✅ |
| tcp-length | 4,792 | 4,762 | 1.0062× | 1.05× | ➖ |
| tcp-eof | 5,307 | 10,231 | 0.5187× | 0.54× | ✅ |
| udp | 7,578 | 7,987 | 0.9488× | 0.95× | ✅ |
| connect | 18,378 | 18,703 | 0.9826× | 0.98× | ✅ |
| https | 108,794 | 108,806 | 0.9999× | 1.0× | ✅ |
| wss | 104,402 | 104,794 | 0.9963× | 1.0× | ✅ |
| atomic | 49,688 | 51,312 | 0.9683× | 0.97× | ✅ |
| process-mode | 16,688 | 16,959 | 0.9840× | 0.99× | ✅ |
| http2-concurrent | 358,862 | 408,880 | 0.8777× | 0.88× | ✅ |
| websocket-64k | 156,025 | 159,695 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 12,825 | 16,328 | 0.7855× | 0.79× | ✅ |
| upload | 478,031 | 1,322,382 | 0.3615× | 0.37× | ✅ |
| static | 29,285 | 41,045 | 0.7135× | 0.72× | ✅ |
| pgsql | 23,209 | 24,595 | 0.9436× | 0.95× | ✅ |
| sqlite | 23,476 | 25,233 | 0.9303× | 0.94× | ✅ |
| curl | 84,850 | 90,350 | 0.9391× | 0.94× | ✅ |
| dns | 230,406 | 232,890 | 0.9893× | 0.99× | ✅ |

Geometric mean 0.8820×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
