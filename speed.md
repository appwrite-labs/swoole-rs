# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `4325896` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,943 | 15,210 | 0.9167× | 0.92× | ✅ |
| echo-1k | 17,045 | 18,162 | 0.9385× | 0.94× | ✅ |
| json | 22,026 | 23,161 | 0.9510× | 0.95× | ✅ |
| service-health | 16,528 | 17,773 | 0.9299× | 0.93× | ✅ |
| websocket | 6,584 | 8,191 | 0.8038× | 0.81× | ✅ |
| timer | 203,395 | 313,520 | 0.6487× | 0.65× | ✅ |
| app | 125,339 | 135,879 | 0.9224× | 0.93× | ✅ |
| http2 | 18,654 | 20,468 | 0.9114× | 0.92× | ✅ |
| table | 195,481 | 201,865 | 0.9684× | 0.97× | ✅ |
| channel | 137,419 | 154,654 | 0.8886× | 0.9× | ✅ |
| task | 31,988 | 33,268 | 0.9615× | 0.96× | ✅ |
| mysql | 49,495 | 51,061 | 0.9693× | 0.97× | ✅ |
| http-client | 33,643 | 35,808 | 0.9395× | 0.94× | ✅ |
| client | 18,439 | 19,757 | 0.9333× | 0.94× | ✅ |
| socket | 19,566 | 21,067 | 0.9287× | 0.92× | ✅ |
| gzip | 194,148 | 197,079 | 0.9851× | 0.99× | ✅ |
| websocket-deflate | 179,387 | 181,568 | 0.9880× | 0.99× | ✅ |
| tcp-length | 4,792 | 4,765 | 1.0055× | 1.01× | ➖ |
| tcp-eof | 5,307 | 10,231 | 0.5187× | 0.53× | ✅ |
| udp | 7,576 | 7,987 | 0.9486× | 0.95× | ✅ |
| connect | 18,378 | 18,703 | 0.9826× | 0.98× | ✅ |
| https | 108,806 | 108,810 | 1.0000× | 1.0× | ✅ |
| wss | 104,405 | 104,801 | 0.9962× | 1.0× | ✅ |
| atomic | 49,684 | 51,316 | 0.9682× | 0.97× | ✅ |
| process-mode | 16,687 | 16,963 | 0.9837× | 0.99× | ✅ |
| http2-concurrent | 360,157 | 408,877 | 0.8808× | 0.88× | ✅ |
| websocket-64k | 156,031 | 159,694 | 0.9771× | 0.98× | ✅ |
| websocket-fragmented | 12,815 | 16,331 | 0.7847× | 0.79× | ✅ |
| upload | 478,085 | 1,322,382 | 0.3615× | 0.37× | ✅ |
| static | 29,347 | 41,074 | 0.7145× | 0.72× | ✅ |
| pgsql | 23,216 | 24,595 | 0.9439× | 0.95× | ✅ |
| sqlite | 23,483 | 25,236 | 0.9305× | 0.94× | ✅ |
| curl | 84,860 | 90,353 | 0.9392× | 0.94× | ✅ |
| dns | 230,570 | 232,965 | 0.9897× | 0.99× | ✅ |

Geometric mean 0.8819×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
