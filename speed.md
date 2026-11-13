# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `5dc72c2` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,942 | 15,214 | 0.9164× | 0.92× | ✅ |
| echo-1k | 17,056 | 18,165 | 0.9390× | 0.94× | ✅ |
| json | 22,043 | 23,161 | 0.9517× | 0.95× | ✅ |
| service-health | 16,536 | 17,777 | 0.9302× | 0.93× | ✅ |
| websocket | 6,553 | 8,185 | 0.8005× | 0.81× | ✅ |
| timer | 203,090 | 313,550 | 0.6477× | 0.65× | ✅ |
| app | 125,380 | 135,932 | 0.9224× | 0.93× | ✅ |
| http2 | 18,595 | 20,467 | 0.9086× | 0.92× | ✅ |
| table | 195,514 | 201,865 | 0.9685× | 0.97× | ✅ |
| channel | 137,452 | 154,654 | 0.8888× | 0.89× | ✅ |
| task | 32,008 | 33,264 | 0.9622× | 0.96× | ✅ |
| mysql | 49,488 | 51,061 | 0.9692× | 0.97× | ✅ |
| http-client | 33,704 | 35,808 | 0.9412× | 0.94× | ✅ |
| client | 18,491 | 19,753 | 0.9361× | 0.94× | ✅ |
| socket | 19,618 | 21,067 | 0.9312× | 0.92× | ✅ |
| gzip | 194,146 | 197,079 | 0.9851× | 0.99× | ✅ |
| websocket-deflate | 179,341 | 181,568 | 0.9877× | 0.99× | ✅ |
| tcp-length | 4,767 | 4,762 | 1.0010× | 1.01× | ➖ |
| tcp-eof | 5,290 | 10,231 | 0.5170× | 0.52× | ✅ |
| udp | 7,571 | 7,986 | 0.9480× | 0.95× | ✅ |
| connect | 17,855 | 18,707 | 0.9545× | 0.98× | ✅ |
| https | 108,635 | 108,803 | 0.9985× | 1.0× | ✅ |
| wss | 104,210 | 104,794 | 0.9944× | 1.0× | ✅ |
| atomic | 49,717 | 51,316 | 0.9689× | 0.97× | ✅ |
| process-mode | 15,455 | 16,960 | 0.9112× | 0.94× | ✅ |
| http2-concurrent | 357,353 | 408,882 | 0.8740× | 0.88× | ✅ |
| websocket-64k | 156,000 | 159,698 | 0.9768× | 0.98× | ✅ |
| websocket-fragmented | 12,785 | 16,331 | 0.7829× | 0.79× | ✅ |
| upload | 477,947 | 1,322,382 | 0.3614× | 0.37× | ✅ |
| static | 28,433 | 40,987 | 0.6937× | 0.72× | ✅ |
| pgsql | 23,229 | 24,594 | 0.9445× | 0.95× | ✅ |
| sqlite | 23,494 | 25,236 | 0.9310× | 0.94× | ✅ |
| curl | 85,729 | 90,482 | 0.9475× | 0.94× | ✅ |
| dns | 230,725 | 232,887 | 0.9907× | 0.99× | ✅ |

Geometric mean 0.8781×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
