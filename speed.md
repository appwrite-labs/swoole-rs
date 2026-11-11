# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `af9311a` (`benchmarks/callgrind.py`, 1000 requests). Ratio is the Cargo build ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits/`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | cargo | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 14,234 | 15,214 | 0.9356× | 0.93× | ✅ |
| echo-1k | 17,360 | 18,165 | 0.9557× | 0.95× | ✅ |
| json | 22,334 | 23,161 | 0.9643× | 0.96× | ✅ |
| service-health | 16,836 | 17,777 | 0.9471× | 0.94× | ✅ |
| websocket | 6,749 | 8,191 | 0.8239× | 0.82× | ✅ |
| timer | 202,416 | 314,540 | 0.6435× | 0.65× | ✅ |
| app | 126,362 | 135,964 | 0.9294× | 0.93× | ✅ |
| http2 | 20,004 | 20,464 | 0.9775× | 0.98× | ✅ |
| table | 196,025 | 201,865 | 0.9711× | 0.97× | ✅ |
| channel | 138,453 | 154,654 | 0.8952× | 0.91× | ✅ |
| task | 32,235 | 33,260 | 0.9692× | 0.97× | ✅ |
| mysql | 49,887 | 51,058 | 0.9771× | 0.98× | ✅ |
| http-client | 33,834 | 35,809 | 0.9448× | 0.95× | ✅ |
| client | 18,770 | 19,757 | 0.9501× | 0.95× | ✅ |
| socket | 19,737 | 21,067 | 0.9369× | 0.94× | ✅ |
| gzip | 196,206 | 197,083 | 0.9955× | 1.0× | ✅ |
| websocket-deflate | 181,504 | 181,568 | 0.9996× | 1.0× | ✅ |
| tcp-length | 5,239 | 4,762 | 1.1001× | 1.09× | ❌ |
| tcp-eof | 5,777 | 10,228 | 0.5648× | 0.56× | ✅ |
| udp | 7,997 | 7,987 | 1.0012× | 1.0× | ❌ |
| connect | 18,473 | 18,707 | 0.9875× | 0.99× | ✅ |
| https | 109,133 | 108,810 | 1.0030× | 1.01× | ➖ |
| wss | 104,710 | 104,797 | 0.9992× | 1.0× | ✅ |
| atomic | 49,976 | 51,316 | 0.9739× | 0.98× | ✅ |
| process-mode | 16,633 | 16,960 | 0.9807× | 0.98× | ✅ |
| http2-concurrent | 374,701 | 408,877 | 0.9164× | 0.92× | ✅ |
| websocket-64k | 156,201 | 159,692 | 0.9781× | 0.98× | ✅ |
| websocket-fragmented | 13,009 | 16,331 | 0.7966× | 0.79× | ✅ |
| upload | 544,411 | 1,322,382 | 0.4117× | 0.42× | ✅ |
| static | 34,346 | 41,010 | 0.8375× | 0.84× | ✅ |
| pgsql | 23,605 | 24,594 | 0.9598× | 0.96× | ✅ |
| sqlite | 23,835 | 25,232 | 0.9446× | 0.95× | ✅ |
| curl | 85,710 | 90,508 | 0.9470× | 0.95× | ✅ |
| dns | 231,437 | 233,156 | 0.9926× | 1.0× | ✅ |

Geometric mean 0.9049×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
