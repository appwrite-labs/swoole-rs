<?php
// A warning's handler replaces $read while the hooked stream_select walks it:
// with [], or with an array another variable holds. That variable keeps its
// elements and the call returns.
Swoole\Runtime::enableCoroutine(SWOOLE_HOOK_ALL);

function select_replacing(array $read, array $replacement): array
{
    $write = $except = null;
    set_error_handler(function () use (&$read, $replacement): bool {
        $read = $replacement;
        return true;
    });
    $ready = stream_select($read, $write, $except, 0);
    restore_error_handler();
    return [$ready, names($read)];
}

function names(array $array): array
{
    return array_map(fn($value) => is_resource($value) ? 'stream' : $value, $array);
}

$result = [];
Co\run(function () use (&$result): void {
    [$a, $b] = stream_socket_pair(STREAM_PF_UNIX, STREAM_SOCK_STREAM, STREAM_IPPROTO_IP);
    $result['empty'] = select_replacing(['not a stream', $a], []);
    $shared = [$b];
    $result['shared'] = [...select_replacing(['not a stream', $a], $shared), names($shared)];
    // $a has buffered data: $read answers with only those streams.
    fwrite($b, 'xy');
    fread($a, 1);
    $buffered = [$a];
    $result['buffered'] = [...select_replacing(['not a stream', $a], $buffered), names($buffered)];
});
echo json_encode($result), "\n";
