<?php
// memory_limit is reached inside an allocation the extension makes for a
// method's result (a string buffer, then the array holding it). Output and
// exit code must match upstream's.
// Usage: php memory_limit.php <case>
$case = $argv[1];
register_shutdown_function(fn() => print("shutdown\n"));

// The next allocation of more than $headroom bytes exceeds the limit.
function limit(int $headroom): void {
    if (ini_set('memory_limit', (string)(memory_get_usage(true) + $headroom)) === false) {
        throw new RuntimeException('cannot set memory_limit');
    }
}

$cases = [
    // One string buffer past the limit.
    'socket_peek' => function () {
        [$socket, $peer] = swoole_coroutine_socketpair(STREAM_PF_UNIX, STREAM_SOCK_STREAM, 0);
        $peer->send('x');
        limit(4 << 20);
        return $socket->peek(6 << 20);
    },
    // Buffers held while the last one goes past the limit.
    'socket_read_vector' => function () {
        [$socket, $peer] = swoole_coroutine_socketpair(STREAM_PF_UNIX, STREAM_SOCK_STREAM, 0);
        $peer->send('x');
        limit(4 << 20);
        return $socket->readVector([1 << 20, 1 << 20, 6 << 20]);
    },
];

Swoole\Coroutine\run(function () use ($cases, $case) {
    $result = $cases[$case]();
    echo "returned\n";
    var_dump(is_string($result) ? strlen($result) : $result);
});
echo "after\n";
