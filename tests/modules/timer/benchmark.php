<?php
// Same PHP workload for reference and opt-in timer variants; measure inside PHP.
[$script, $mode] = $argv;
$n = 1000;
$callbacks = 0;
$errors = 0;
$elapsed = [];
$started = hrtime(true);
if ($mode === 'cancel') {
    for ($round = 0; $round < 100; $round++) {
        $ids = [];
        for ($i = 0; $i < $n; $i++) {
            $id = Swoole\Timer::after(60000, static function () use (&$errors) { $errors++; });
            if ($id === false) $errors++;
            $ids[] = $id;
        }
        // Both middle and root removals; neither queue gets an ordered-drain shortcut.
        foreach (array_merge(array_reverse(array_slice($ids, 0, 500)), array_slice($ids, 500)) as $id) {
            if (!Swoole\Timer::clear($id)) $errors++;
        }
        if (Swoole\Timer::stats()['num'] !== 0) $errors++;
    }
    Swoole\Event::wait();
    $operations = $n * 100;
} elseif ($mode === 'dispatch') {
    for ($round = 0; $round < 20; $round++) {
        for ($i = 0; $i < $n; $i++) {
            $registered = hrtime(true);
            if (Swoole\Timer::after(1, static function () use (&$callbacks, &$elapsed, $registered) {
                $callbacks++;
                $elapsed[] = (hrtime(true) - $registered) / 1e6;
            }) === false) $errors++;
        }
        Swoole\Event::wait();
    }
    $operations = $n * 20;
    if ($callbacks !== $operations) $errors++;
} elseif ($mode === 'repeat') {
    for ($i = 0; $i < $n; $i++) {
        $registered = hrtime(true);
        if (Swoole\Timer::tick(1, static function ($id) use (&$callbacks, &$elapsed, &$errors, $registered) {
            static $counts = [];
            $callbacks++;
            $counts[$id] = ($counts[$id] ?? 0) + 1;
            if ($counts[$id] === 5) {
                $elapsed[] = (hrtime(true) - $registered) / 1e6;
                if (!Swoole\Timer::clear($id)) $errors++;
                unset($counts[$id]);
            }
        }) === false) $errors++;
    }
    Swoole\Event::wait();
    $operations = $n * 5;
    if ($callbacks !== $operations) $errors++;
} else {
    throw new InvalidArgumentException('unknown timer workload');
}
$seconds = (hrtime(true) - $started) / 1e9;
if (Swoole\Timer::stats()['num'] !== 0) $errors++;
sort($elapsed);
$percentiles = [];
foreach (['p50' => .5, 'p95' => .95, 'p99' => .99] as $label => $fraction) {
    $percentiles[$label] = $elapsed ? $elapsed[(int) floor((count($elapsed) - 1) * $fraction)] : null;
}
echo json_encode(['mode' => $mode, 'operations' => $operations, 'callbacks' => $callbacks,
    'errors' => $errors, 'seconds' => $seconds, 'operations_per_second' => $operations / $seconds,
    'callback_elapsed_ms' => $percentiles], JSON_THROW_ON_ERROR), "\n";
exit($errors ? 1 : 0);
