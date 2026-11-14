<?php
$ticks = 0;
$events = [];
$cancelled = Swoole\Timer::after(1000, static function () use (&$events): void {
    $events[] = 'cancelled timer fired';
});
$existsBefore = Swoole\Timer::exists($cancelled);
$cleared = Swoole\Timer::clear($cancelled);
$existsAfter = Swoole\Timer::exists($cancelled);
Swoole\Timer::tick(1, static function ($timerId) use (&$ticks, &$events): void {
    $ticks++;
    if ($ticks === 3) {
        Swoole\Timer::clear($timerId);
        // A nested one-shot runs on the same event loop after the repeat ends.
        Swoole\Timer::after(1, static function () use (&$events): void {
            $events[] = 'done';
        });
    }
});
Swoole\Event::wait();
echo json_encode(compact('ticks', 'events', 'existsBefore', 'cleared', 'existsAfter')), "\n";
