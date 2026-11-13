# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `c0f0972` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,940 | 15,214 | 0.9163× | 0.92× | ✅ |
| echo-1k | 17,044 | 18,165 | 0.9383× | 0.94× | ✅ |
| json | 22,024 | 23,161 | 0.9509× | 0.95× | ✅ |
| service-health | 16,522 | 17,777 | 0.9294× | 0.93× | ✅ |
| websocket | 6,586 | 8,188 | 0.8043× | 0.82× | ✅ |
| timer | 203,487 | 313,550 | 0.6490× | 0.65× | ✅ |
| app | 125,663 | 135,929 | 0.9245× | 0.93× | ✅ |
| http2 | 18,653 | 20,464 | 0.9115× | 0.93× | ✅ |
| table | 195,479 | 201,865 | 0.9684× | 0.97× | ✅ |
| channel | 138,446 | 154,651 | 0.8952× | 0.9× | ✅ |
| task | 32,125 | 33,265 | 0.9657× | 0.96× | ✅ |
| mysql | 49,521 | 51,062 | 0.9698× | 0.97× | ✅ |
| http-client | 33,710 | 35,805 | 0.9415× | 0.94× | ✅ |
| client | 18,448 | 19,757 | 0.9338× | 0.94× | ✅ |
| socket | 19,584 | 21,067 | 0.9296× | 0.92× | ✅ |
| gzip | 194,143 | 197,079 | 0.9851× | 1.0× | ✅ |
| websocket-deflate | 179,406 | 181,572 | 0.9881× | 1.0× | ✅ |
| tcp-length | 4,797 | 4,765 | 1.0066× | 1.05× | ➖ |
| tcp-eof | 5,345 | 10,231 | 0.5224× | 0.54× | ✅ |
| udp | 7,572 | 7,987 | 0.9480× | 0.95× | ✅ |
| connect | 18,360 | 18,707 | 0.9815× | 0.98× | ✅ |
| https | 108,800 | 108,810 | 0.9999× | 1.0× | ✅ |
| wss | 104,496 | 104,794 | 0.9972× | 1.0× | ✅ |
| atomic | 49,686 | 51,316 | 0.9682× | 0.97× | ✅ |
| process-mode | 16,702 | 17,025 | 0.9810× | 0.99× | ✅ |
| http2-concurrent | 359,219 | 408,877 | 0.8786× | 0.88× | ✅ |
| websocket-64k | 156,029 | 159,694 | 0.9771× | 0.98× | ✅ |
| websocket-fragmented | 12,830 | 16,327 | 0.7858× | 0.79× | ✅ |
| upload | 478,023 | 1,322,380 | 0.3615× | 0.37× | ✅ |
| static | 29,321 | 41,074 | 0.7139× | 0.72× | ✅ |
| pgsql | 23,208 | 24,596 | 0.9436× | 0.95× | ✅ |
| sqlite | 23,492 | 25,221 | 0.9314× | 0.94× | ✅ |
| curl | 84,851 | 90,483 | 0.9378× | 0.94× | ✅ |
| dns | 231,025 | 232,720 | 0.9927× | 0.99× | ✅ |

Geometric mean 0.8825×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
