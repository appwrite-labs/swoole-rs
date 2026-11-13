# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `699175b` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,870 | 15,210 | 0.9119× | 0.92× | ✅ |
| echo-1k | 16,970 | 18,162 | 0.9344× | 0.94× | ✅ |
| json | 21,969 | 23,158 | 0.9487× | 0.95× | ✅ |
| service-health | 16,468 | 17,777 | 0.9264× | 0.93× | ✅ |
| websocket | 6,479 | 8,188 | 0.7913× | 0.81× | ✅ |
| timer | 203,417 | 314,112 | 0.6476× | 0.65× | ✅ |
| app | 125,350 | 135,975 | 0.9219× | 0.93× | ✅ |
| http2 | 18,476 | 20,464 | 0.9028× | 0.92× | ✅ |
| table | 195,442 | 201,865 | 0.9682× | 0.97× | ✅ |
| channel | 137,380 | 154,654 | 0.8883× | 0.89× | ✅ |
| task | 31,923 | 33,268 | 0.9596× | 0.96× | ✅ |
| mysql | 49,466 | 51,056 | 0.9689× | 0.97× | ✅ |
| http-client | 33,627 | 35,800 | 0.9393× | 0.94× | ✅ |
| client | 18,405 | 19,757 | 0.9316× | 0.94× | ✅ |
| socket | 19,533 | 21,073 | 0.9270× | 0.92× | ✅ |
| gzip | 194,070 | 197,083 | 0.9847× | 0.99× | ✅ |
| websocket-deflate | 179,309 | 181,572 | 0.9875× | 0.99× | ✅ |
| tcp-length | 4,690 | 4,762 | 0.9849× | 1.01× | ✅ |
| tcp-eof | 5,213 | 10,231 | 0.5095× | 0.52× | ✅ |
| udp | 7,378 | 7,986 | 0.9239× | 0.95× | ✅ |
| connect | 17,539 | 18,707 | 0.9376× | 0.98× | ✅ |
| https | 108,014 | 108,803 | 0.9927× | 1.0× | ✅ |
| wss | 103,634 | 104,794 | 0.9889× | 1.0× | ✅ |
| atomic | 49,578 | 51,312 | 0.9662× | 0.97× | ✅ |
| process-mode | 14,899 | 16,957 | 0.8787× | 0.94× | ✅ |
| http2-concurrent | 357,936 | 408,883 | 0.8754× | 0.88× | ✅ |
| websocket-64k | 155,926 | 159,696 | 0.9764× | 0.98× | ✅ |
| websocket-fragmented | 12,700 | 16,328 | 0.7778× | 0.79× | ✅ |
| upload | 477,839 | 1,322,380 | 0.3613× | 0.37× | ✅ |
| static | 27,281 | 40,981 | 0.6657× | 0.72× | ✅ |
| pgsql | 23,141 | 24,595 | 0.9409× | 0.95× | ✅ |
| sqlite | 23,411 | 25,236 | 0.9277× | 0.94× | ✅ |
| curl | 85,646 | 90,471 | 0.9467× | 0.94× | ✅ |
| dns | 230,933 | 233,201 | 0.9903× | 0.99× | ✅ |

Geometric mean 0.8721×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
