# Speed vs upstream Swoole

Callgrind instructions per request (lower is better), measured by the merge tier on `e79e35b` (`benchmarks/callgrind.py`, 1000 requests). Ratio is hybrid ÷ upstream: below 1.00× is faster than upstream. The limit is `benchmarks/callgrind_limits.json`; ✅ beats upstream, ➖ within the limit, ❌ over it.

| workload | hybrid | upstream | ratio | limit | |
| --- | ---: | ---: | ---: | ---: | --- |
| plaintext | 13,835 | 15,203 | 0.9100× | 0.91× | ✅ |
| echo-1k | 16,692 | 18,148 | 0.9197× | 0.92× | ✅ |
| json | 21,780 | 23,163 | 0.9403× | 0.95× | ✅ |
| service-health | 16,412 | 17,795 | 0.9223× | 0.93× | ✅ |
| websocket | 6,574 | 8,191 | 0.8025× | 0.81× | ✅ |
| timer | 226,723 | 314,198 | 0.7216× | 0.68× | ✅ |
| app | 125,160 | 135,958 | 0.9206× | 0.93× | ✅ |
| http2 | 19,272 | 20,346 | 0.9472× | 0.95× | ✅ |
| table | 197,713 | 201,685 | 0.9803× | 0.99× | ✅ |
| channel | 136,502 | 154,568 | 0.8831× | 0.89× | ✅ |
| task | 31,107 | 33,184 | 0.9374× | 0.94× | ✅ |
| mysql | 49,088 | 51,017 | 0.9622× | 0.97× | ✅ |

Geometric mean 0.9010×. Goal: ≤ 1.00× on every workload ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)).
