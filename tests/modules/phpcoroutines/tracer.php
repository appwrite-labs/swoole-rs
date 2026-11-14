<?php
// The tracer's observable output (no upstream PHPT covers it); tracer.py runs
// each mode under its INI switch on both engines and compares normalized output.
$mode = $argv[1];

function show($label, callable $fn) {
    try {
        echo $label, ': ', var_export($fn(), true), "\n";
    } catch (Throwable $e) {
        echo $label, ': ', get_class($e), ': ', $e->getMessage(), "\n";
    }
}

class Shape {
    public function area($n) { return helper($n) * 2; }
    public static function make() { return new Shape; }
}

function helper($n) { return $n + 1; }

function leak(array &$keep, $n) {
    for ($i = 0; $i < $n; $i++) {
        $keep[] = str_repeat('x', 100 + $i);
    }
}

switch ($mode) {
case 'off':
    show('leak_detect', fn() => swoole_tracer_leak_detect());
    show('prof_begin', fn() => swoole_tracer_prof_begin());
    show('prof_end', fn() => swoole_tracer_prof_end('/dev/null'));
    break;

case 'profile':
    $file = tempnam(sys_get_temp_dir(), 'trace');
    show('prof_end before begin', fn() => swoole_tracer_prof_end($file));
    show('prof_begin bad', fn() => swoole_tracer_prof_begin('x'));
    show('prof_begin again', fn() => swoole_tracer_prof_begin(['root_path' => __DIR__]));
    $shape = Shape::make();
    $shape->area(1);
    (function () { return helper(2); })();
    Swoole\Coroutine\run(function () use ($shape) {
        go(function () use ($shape) { $shape->area(3); });
        helper(4);
    });
    show('prof_end bad', fn() => swoole_tracer_prof_end([]));
    show('prof_end unwritable', fn() => swoole_tracer_prof_end('/nonexistent/dir/trace.json'));
    show('prof_end', fn() => swoole_tracer_prof_end($file));
    $trace = json_decode(file_get_contents($file), true);
    unlink($file);
    echo json_encode($trace['viztracer_metadata']), "\n";
    foreach ($trace['traceEvents'] as $event) {
        $ok = is_numeric($event['ts']) && is_numeric($event['dur']) && $event['dur'] >= 0 && is_int($event['pid']);
        echo $event['name'], ' ', $event['ph'], ' ', $event['cat'], ' tid=', $event['tid'], $ok ? '' : ' BAD', "\n";
    }
    show('prof_end after end', fn() => swoole_tracer_prof_end($file));
    show('prof_begin root_path', fn() => swoole_tracer_prof_begin(['root_path' => __DIR__]));
    helper(5);
    show('prof_end', fn() => swoole_tracer_prof_end($file));
    foreach (json_decode(file_get_contents($file), true)['traceEvents'] as $event) {
        echo $event['name'], "\n";
    }
    unlink($file);
    break;

case 'leak':
    show('leak_detect bad', fn() => swoole_tracer_leak_detect('x'));
    $keep = [];
    leak($keep, 100);
    swoole_tracer_leak_detect(50);
    echo "-- end\n";
    break;

case 'blocking':
    password_hash('outside', PASSWORD_BCRYPT, ['cost' => 10]);
    Swoole\Coroutine\run(function () {
        password_hash('inside', PASSWORD_BCRYPT, ['cost' => 10]);
        Swoole\Coroutine::sleep(0.05);
        helper(1);
    });
    echo "-- end\n";
    break;
}
