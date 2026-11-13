# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `951fd4e` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,966 | 15,214 | 0.9180× | 0.92× | ✅ |
| echo-1k | 17,080 | 18,162 | 0.9404× | 0.94× | ✅ |
| json | 22,066 | 23,158 | 0.9529× | 0.95× | ✅ |
| service-health | 16,565 | 17,773 | 0.9320× | 0.93× | ✅ |
| websocket | 6,581 | 8,188 | 0.8038× | 0.81× | ✅ |
| timer | 203,495 | 313,598 | 0.6489× | 0.65× | ✅ |
| app | 125,400 | 135,914 | 0.9226× | 0.93× | ✅ |
| http2 | 18,681 | 20,465 | 0.9128× | 0.92× | ✅ |
| table | 195,535 | 201,865 | 0.9686× | 0.97× | ✅ |
| channel | 137,473 | 154,654 | 0.8889× | 0.89× | ✅ |
| task | 32,040 | 33,267 | 0.9631× | 0.96× | ✅ |
| mysql | 49,600 | 51,064 | 0.9713× | 0.97× | ✅ |
| http-client | 33,739 | 35,801 | 0.9424× | 0.94× | ✅ |
| client | 18,515 | 19,757 | 0.9371× | 0.94× | ✅ |
| socket | 19,642 | 21,067 | 0.9323× | 0.92× | ✅ |
| gzip | 194,163 | 197,079 | 0.9852× | 0.99× | ✅ |
| websocket-deflate | 179,383 | 181,568 | 0.9880× | 0.99× | ✅ |
| tcp-length | 4,791 | 4,763 | 1.0058× | 1.01× | ➖ |
| tcp-eof | 5,314 | 10,231 | 0.5194× | 0.52× | ✅ |
| udp | 7,576 | 7,986 | 0.9486× | 0.95× | ✅ |
| connect | 18,409 | 18,707 | 0.9841× | 0.98× | ✅ |
| https | 108,791 | 108,806 | 0.9999× | 1.0× | ✅ |
| wss | 104,387 | 104,793 | 0.9961× | 1.0× | ✅ |
| atomic | 49,749 | 51,316 | 0.9695× | 0.97× | ✅ |
| process-mode | 15,876 | 16,956 | 0.9363× | 0.94× | ✅ |
| http2-concurrent | 359,663 | 408,882 | 0.8796× | 0.88× | ✅ |
| websocket-64k | 156,030 | 159,699 | 0.9770× | 0.98× | ✅ |
| websocket-fragmented | 12,800 | 16,327 | 0.7840× | 0.79× | ✅ |
| upload | 478,124 | 1,322,383 | 0.3616× | 0.37× | ✅ |
| static | 29,159 | 41,045 | 0.7104× | 0.72× | ✅ |
| pgsql | 23,241 | 24,594 | 0.9450× | 0.95× | ✅ |
| sqlite | 23,510 | 25,236 | 0.9316× | 0.94× | ✅ |
| curl | 84,885 | 90,482 | 0.9381× | 0.94× | ✅ |
| dns | 230,323 | 232,563 | 0.9904× | 0.99× | ✅ |

Geometric mean 0.8812×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
