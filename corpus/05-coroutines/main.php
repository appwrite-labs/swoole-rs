<?php
use Swoole\Coroutine\Channel;
use Swoole\Coroutine\System;
use function Swoole\Coroutine\go;
use function Swoole\Coroutine\run;

run(static function (): void {
    $channel = new Channel(2);
    foreach ([1, 2, 3, 4] as $id) {
        go(static function () use ($id, $channel): void {
            System::sleep(0.01);
            $channel->push(['id' => $id, 'result' => $id * $id]);
        });
    }
    $results = [];
    for ($i = 0; $i < 4; $i++) {
        $results[] = $channel->pop(1.0);
    }
    usort($results, static fn($a, $b) => $a['id'] <=> $b['id']);
    $empty = $channel->pop(0.01);
    echo json_encode(['results' => $results, 'timed_out' => $empty === false]), PHP_EOL;
});
