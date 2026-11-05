<?php
$mode = $argv[1] ?? 'plain';
if ($mode === 'coroutine') {
    if (Swoole\Thread::getInfo()['is_main_thread']) {
        $observed = new Swoole\Thread\Atomic(0);
        $worker = new Swoole\Thread(__FILE__, $mode, $observed);
        if (!$worker->join() || $worker->getExitStatus() !== 235 || $observed->get() !== 1) {
            throw new RuntimeException('native C++ coroutine relay changed thread lifecycle');
        }
        echo "cpp-chain: ok\n";
        exit;
    }
    [$mode, $observed] = Swoole\Thread::getArguments();
}
$result = cpp_chain(function () use ($mode) {
    if ($mode === 'coroutine') {
        Swoole\Coroutine\run(function () {
            go(function () {
                cpp_native_catch();
                cpp_publish();
            });
            cpp_publish();
        });
    } else {
        cpp_native_catch();
        cpp_publish();
    }
});
$expected = [3, 0, 2, 8192, 8192 * 42, 1, true];
if ($result !== $expected) {
    fwrite(STDERR, json_encode(['expected' => $expected, 'observed' => $result]) . "\n");
    exit(1);
}
if ($mode === 'coroutine') {
    // The test's outer native frame has handled/released its C++ payload.
    // End this request before its intentionally parked coroutine can resume.
    $observed->set(1);
    swoole_implicit_fn('bailout', 235);
    throw new LogicException('request continued after outer native boundary');
}
echo "cpp-chain: ok\n";
