<?php
use Swoole\Thread;
if (($argv[1] ?? '') === 'cpp-probe') {
    if (!swr_guard_cpp_probe()) throw new RuntimeException('native C++ guard cleanup failed');
    echo "cpp-probe: ok\n";
    exit;
}
function guarded(callable $callback) {
    return function_exists('swr_guard_owned') ? swr_guard_owned($callback) : $callback();
}
function escape(): never { swoole_implicit_fn('bailout', 235); }
class ExitDestructor {
    public function __destruct() { escape(); }
}
class ExitSerialization {
    public function __serialize(): array {
        if (in_array($GLOBALS['mode'], ['serialize', 'shared-set', 'shared-queue'], true)) guarded(fn() => escape());
        return [];
    }
    public function __unserialize(array $data): void {
        guarded(fn() => escape());
    }
}
if (Thread::getInfo()['is_main_thread']) {
    $mode = $argv[1];
    $after = new Thread\Atomic(0);
    $map = new Thread\Map(['kept' => 'value']);
    $queue = new Thread\Queue();
    $worker = new Thread(__FILE__, $mode, $after, $map, $queue);
    if (!$worker->join()) throw new RuntimeException('join failed');
    $normal = in_array($mode, ['return', 'exception'], true);
    $expected = $normal ? 0 : ($mode === 'fatal' ? 255 : 235);
    if ($worker->getExitStatus() !== $expected || $after->get() !== ($normal ? 1 : 0)) {
        throw new RuntimeException('exit or post-callback behavior changed');
    }
    if ($map['kept'] !== 'value' || $queue->count() !== 0) {
        throw new RuntimeException('failed operation changed shared state');
    }
    if (function_exists('swr_guard_counts')) {
        $count = in_array($mode, ['nested', 'coroutine', 'cancel', 'cancel-throw', 'serialize', 'unserialize', 'values', 'shared-set', 'shared-queue'], true) ? 2 : 1;
        if (swr_guard_counts() !== [$count, $count, $count, $count * 4096, $count * 4096 * 42, $mode === 'fatal' ? 1 : 0, $normal || in_array($mode, ['fatal', 'exit'], true) ? 0 : $count]) {
            throw new RuntimeException('Rust owner cleanup or native outcome provenance changed');
        }
    }
    echo json_encode([$mode, $worker->getExitStatus(), $after->get()]), "\n";
    exit;
}
[$mode, $after, $map, $queue] = Thread::getArguments();
if ($mode === 'exception') {
    try {
        guarded(function () { throw new RuntimeException('kept'); });
        throw new LogicException('exception was swallowed');
    } catch (RuntimeException $exception) {
        if ($exception->getMessage() !== 'kept') throw $exception;
    }
} elseif ($mode === 'nested') {
    guarded(fn() => guarded(fn() => escape()));
} elseif ($mode === 'coroutine') {
    Swoole\Coroutine\run(function () {
        guarded(function () {
            go(fn() => guarded(fn() => escape()));
            throw new LogicException('resumer continued after whole-thread exit');
        });
    });
} elseif (in_array($mode, ['cancel', 'cancel-throw'], true)) {
    Swoole\Coroutine\run(function () use ($mode) {
        guarded(function () use ($mode) {
            $cid = go(function () use ($mode) {
                defer(fn() => guarded(fn() => escape()));
                try { Swoole\Coroutine::yield(); }
                catch (Swoole\Coroutine\CanceledException $exception) {
                    if ($mode !== 'cancel-throw') throw $exception;
                }
            });
            Swoole\Coroutine::cancel($cid, $mode === 'cancel-throw');
            throw new LogicException('cancellation resumer continued after whole-thread exit');
        });
    });
} elseif ($mode === 'fatal') {
    error_reporting(0);
    guarded(fn() => trigger_error('guard fatal probe', E_USER_ERROR));
} elseif ($mode === 'error') {
    set_error_handler(fn() => escape());
    guarded(fn() => trigger_error('exit handler'));
} elseif ($mode === 'destructor') {
    guarded(function () { $object = new ExitDestructor(); unset($object); });
} elseif ($mode === 'serialize') {
    guarded(fn() => new Thread\Map(['object' => new ExitSerialization()]));
} elseif ($mode === 'unserialize') {
    $map = new Thread\Map(['object' => new ExitSerialization()]);
    guarded(fn() => $map['object']);
} elseif ($mode === 'values') {
    // The first item's escape ends the copy: the second is never unserialized.
    $map = new Thread\Map(['first' => new ExitSerialization(), 'second' => new ExitSerialization()]);
    guarded(fn() => $map->values());
} elseif ($mode === 'shared-set') {
    guarded(function () use ($map) { $map['kept'] = new ExitSerialization(); });
} elseif ($mode === 'shared-queue') {
    guarded(fn() => $queue->push(new ExitSerialization()));
} elseif ($mode === 'exit') {
    guarded(fn() => exit(235));
} elseif ($mode === 'return') {
    guarded(fn() => 42);
} else {
    guarded(fn() => escape());
}
$after->add();
