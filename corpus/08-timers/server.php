<?php
// A timer-heavy HTTP endpoint for the callgrind gate: each request adds and then
// clears a fixed batch of far-future timers, so the timer module's add/remove paths
// dominate while the instruction count stays deterministic (no timer ever fires, so
// there is no event-loop timing to vary between runs). The CLI timer scenarios live
// in main.php; this file exists only for benchmarks/callgrind.py.
$server = new Swoole\Http\Server('127.0.0.1', (int) getenv('TEST_PORT'));
$server->set(['worker_num' => 1]);
$noop = static function (): void {};
$server->on('request', static function ($request, $response) use ($noop): void {
    $ids = [];
    for ($i = 0; $i < 100; $i++) {
        $ids[] = Swoole\Timer::after(3600000, $noop);
    }
    foreach ($ids as $id) {
        Swoole\Timer::clear($id);
    }
    $response->end("timers\n");
});
$server->start();
