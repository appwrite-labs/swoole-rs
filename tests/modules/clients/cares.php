<?php
function check(bool $condition, string $message): void {
    if (!$condition) throw new RuntimeException($message);
}
$mode = $argv[1];
Swoole\Coroutine::set(['dns_server' => $argv[2], 'dns_lookup_random' => false]);
$results = [];
Swoole\Coroutine\run(function() use ($mode, &$results) {
    if ($mode === 'cancel') {
        $id = Swoole\Coroutine::create(function() use (&$results) {
            $value = Swoole\Coroutine\System::dnsLookup('cancel.fixture.test', 2, AF_INET);
            $results[] = [$value, swoole_last_error()];
        });
        Swoole\Coroutine::sleep(0.01);
        check(Swoole\Coroutine::cancel($id), 'cancel suspended query');
    } elseif ($mode === 'immediate') {
        $results[] = [Swoole\Coroutine\System::dnsLookup('127.0.0.42', 1, AF_INET)];
        $results[] = [Swoole\Coroutine\System::dnsLookup('::1', 1, AF_INET6)];
    } else {
        for ($i = 0; $i < ($mode === 'concurrent' ? 32 : 1); $i++) {
            Swoole\Coroutine::create(function() use ($mode, $i, &$results) {
                $v6 = $mode === 'aaaa';
                $value = Swoole\Coroutine\System::dnsLookup("$mode-$i.fixture.test", $mode === 'timeout' ? 0.03 : 2, $v6 ? AF_INET6 : AF_INET);
                $results[] = [$value, $value === false ? swoole_last_error() : 0];
            });
        }
    }
});
if ($mode === 'cancel') check($results === [[false, SWOOLE_ERROR_DNSLOOKUP_RESOLVE_FAILED]], 'cancel outcome:'.json_encode($results));
elseif ($mode === 'timeout') check($results === [[false, SWOOLE_ERROR_DNSLOOKUP_RESOLVE_FAILED]], 'timeout outcome:'.json_encode($results));
elseif ($mode === 'nxdomain') check($results === [[false, SWOOLE_ERROR_DNSLOOKUP_RESOLVE_FAILED]], 'NXDOMAIN outcome');
elseif ($mode === 'immediate') check($results === [['127.0.0.42'], ['0000:0000:0000:0000:0000:0000:0000:0001']], 'synchronous completion');
else {
    check(count($results) === ($mode === 'concurrent' ? 32 : 1), 'all completions delivered once');
    $expected = $mode === 'aaaa' ? '2001:0db8:0000:0000:0000:0000:0000:0042' : '192.0.2.42';
    foreach ($results as $result) check($result === [$expected, 0], 'address result');
}
echo json_encode(['mode' => $mode, 'results' => $results]), "\n";
