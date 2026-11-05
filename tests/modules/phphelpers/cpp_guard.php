<?php
$mode = $argv[1] ?? 'plain';
$control = str_contains($mode, 'catch');
function operation(bool $control): array {
    return cpp_chain(function () {
        cpp_native_catch();
        cpp_publish();
    }, $control);
}
function check(array $result, bool $control): void {
    $expected = [$control ? 0 : 3, $control ? 1 : 0, 2, 8192, 8192 * 42, 1, $control ? 1 : 2, true];
    if ($result !== $expected) {
        fwrite(STDERR, json_encode(['expected' => $expected, 'observed' => $result]) . "\n");
        exit(1);
    }
}
if (str_starts_with($mode, 'coroutine')) {
    Swoole\Coroutine\run(function () use ($control) { check(operation($control), $control); });
} elseif ($mode === 'non-lifo') {
    Swoole\Coroutine\run(function () {
        $first = go(function () {
            $result = cpp_chain(function () {
                Swoole\Coroutine::yield();
                cpp_native_catch();
                cpp_publish();
            });
            check($result, false);
        });
        go(function () use ($first) {
            $result = cpp_chain(function () use ($first) {
                if (!Swoole\Coroutine::resume($first)) throw new RuntimeException('resume failed');
                cpp_native_catch();
                cpp_publish();
            });
            check($result, false);
        });
    });
} else {
    check(operation($control), $control);
}
echo "cpp-chain: ok\n";
