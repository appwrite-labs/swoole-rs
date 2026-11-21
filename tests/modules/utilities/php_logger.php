<?php
// Real binding/FILE/fork witnesses. Every log stays in the supplied private directory.
$root = $argv[1];
function check(bool $condition, string $message): void {
    if (!$condition) { throw new RuntimeException($message); }
}
function configureLog(string $path, int $rotation = SWOOLE_LOG_ROTATION_SINGLE): void {
    // Rotation is applied after log_file by the native binding; set it first.
    swoole_async_set(['log_rotation' => $rotation]);
    swoole_async_set(['log_file' => $path, 'log_level' => SWOOLE_LOG_NOTICE,
        'log_date_format' => 'M14', 'log_date_with_microseconds' => false]);
}
$summary = [];
foreach ([SWOOLE_LOG_ROTATION_SINGLE => 0, SWOOLE_LOG_ROTATION_MONTHLY => 6,
          SWOOLE_LOG_ROTATION_DAILY => 8, SWOOLE_LOG_ROTATION_HOURLY => 10,
          SWOOLE_LOG_ROTATION_EVERY_MINUTE => 12] as $rotation => $digits) {
    $path = "$root/rotation-$rotation";
    configureLog($path, $rotation);
    swoole_error_log(SWOOLE_LOG_NOTICE, "#M14:rotation-$rotation#");
    $files = glob($path . '*');
    check(count($files) === 1, 'one rotation file');
    $suffix = substr($files[0], strlen($path));
    check($digits === 0 ? $suffix === '' : preg_match('/^\.[0-9]{' . $digits . '}$/D', $suffix) === 1,
        'exact rotation suffix width');
    $content = file_get_contents($files[0]);
    check(preg_match('/^\[M14 %[0-9]+\.-?[0-9]+\]\tNOTICE\t#M14:rotation-' . $rotation . '#\n$/D', $content) === 1,
        'exact formatted rotation record');
    $summary['rotation-' . $rotation] = $digits;
}
// strftime's output capacity, not the input length, decides acceptance.
// A rejected format must log through the previous format and leave it usable.
foreach (['' => 0, str_repeat('x', 128) => 128, str_repeat('%Y', 32) => 64] as $format => $length) {
    $path = "$root/rejected-date-$length";
    configureLog($path);
    swoole_async_set(['log_date_format' => $format]);
    check(swoole_last_error() === SWOOLE_ERROR_INVALID_PARAMS, 'rejected date sets last error');
    swoole_error_log(SWOOLE_LOG_NOTICE, '#M14:after-rejection#');
    $lines = file($path);
    check(count($lines) === 2, 'one rejection diagnostic and one later record');
    check(str_starts_with($lines[0], '[M14 %') &&
        str_contains($lines[0], "The date format string[length=$length] is too long"),
        'rejection uses previous format and original input length');
    check(str_starts_with($lines[1], '[M14 %') && str_ends_with($lines[1], "#M14:after-rejection#\n"),
        'rejection preserves format for subsequent emission');
}
configureLog("$root/date-boundary");
swoole_async_set(['log_date_format' => str_repeat('x', 127)]);
swoole_error_log(SWOOLE_LOG_NOTICE, '#M14:date-boundary#');
check(str_starts_with(file_get_contents("$root/date-boundary"), '[' . str_repeat('x', 127) . ' %'),
    '127 output bytes fit the date buffer');
configureLog("$root/date-nul");
swoole_async_set(['log_date_format' => "NUL\0ignored"]);
swoole_error_log(SWOOLE_LOG_NOTICE, '#M14:date-nul#');
check(str_starts_with(file_get_contents("$root/date-nul"), '[NUL %'),
    'native C string format stops at embedded NUL');
$summary['date-validation'] = ['rejected-input-lengths' => [0, 128, 64], 'accepted-output-bytes' => 127,
    'embedded-nul' => true];
configureLog("$root/first");
swoole_error_log(SWOOLE_LOG_NOTICE, '#M14:first#');
configureLog("$root/second");
swoole_error_log(SWOOLE_LOG_NOTICE, '#M14:second#');
check(substr_count(file_get_contents("$root/first"), '#M14:first#') === 1, 'old file retained');
check(!str_contains(file_get_contents("$root/first"), '#M14:second#'), 'closed file receives no later record');
check(substr_count(file_get_contents("$root/second"), '#M14:second#') === 1, 'new file receives record');
$summary['reconfigure'] = 2;
foreach ([-100, SWOOLE_LOG_NOTICE, 100] as $threshold) {
    $path = "$root/level-$threshold"; configureLog($path);
    swoole_async_set(['log_level' => $threshold]);
    foreach ([SWOOLE_LOG_DEBUG, SWOOLE_LOG_TRACE, SWOOLE_LOG_INFO, SWOOLE_LOG_NOTICE,
              SWOOLE_LOG_WARNING, SWOOLE_LOG_ERROR] as $level) {
        swoole_error_log($level, "#M14:level-$level#");
    }
    $content = file_get_contents($path); $expected = [];
    foreach ([SWOOLE_LOG_DEBUG, SWOOLE_LOG_TRACE, SWOOLE_LOG_INFO, SWOOLE_LOG_NOTICE,
              SWOOLE_LOG_WARNING, SWOOLE_LOG_ERROR] as $level) {
        if ($level >= max(SWOOLE_LOG_DEBUG, min(SWOOLE_LOG_NONE, $threshold))) { $expected[] = "level-$level"; }
    }
    preg_match_all('/#M14:([^#]+)#/', $content, $matches);
    check($matches[1] === $expected, 'level clamp/filter/order');
    $summary['level-' . $threshold] = $matches[1];
}
configureLog("$root/automatic");
swoole_error_log(SWOOLE_LOG_NOTICE, '#M14:before-rotation#');
swoole_async_set(['log_rotation' => SWOOLE_LOG_ROTATION_DAILY]);
swoole_error_log(SWOOLE_LOG_NOTICE, '#M14:after-rotation#');
$rotated = glob("$root/automatic.*");
check(count($rotated) === 1, 'put triggers one rotated file');
check(str_ends_with(file_get_contents("$root/automatic"), "#M14:before-rotation#\n"), 'old file closes before rotation');
check(str_ends_with(file_get_contents($rotated[0]), "#M14:after-rotation#\n"), 'reopened file receives new record');
swoole_async_set(['log_rotation' => SWOOLE_LOG_ROTATION_SINGLE]);
swoole_error_log(SWOOLE_LOG_NOTICE, '#M14:rotation-disabled#');
check(str_ends_with(file_get_contents($rotated[0]), "#M14:rotation-disabled#\n"), 'disabling rotation keeps current FILE');
$summary['automatic-rotation'] = 3;
configureLog("$root/micros");
swoole_async_set(['log_date_with_microseconds' => true]);
swoole_error_log(SWOOLE_LOG_NOTICE, '#M14:micros#');
check(preg_match('/^\[M14<\.[0-9]+> %[0-9]+\.-?[0-9]+\]\tNOTICE\t#M14:micros#\n$/D',
    file_get_contents("$root/micros")) === 1, 'microseconds formatting');
$summary['microseconds'] = true;
if (!function_exists('pcntl_fork')) { throw new RuntimeException('UNSUPPORTED: pcntl_fork required'); }
configureLog("$root/fork");
swoole_error_log(SWOOLE_LOG_NOTICE, '#M14:parent-before#');
$pid = pcntl_fork(); check($pid >= 0, 'fork succeeds');
if ($pid === 0) {
    swoole_error_log(SWOOLE_LOG_NOTICE, '#M14:child-inherited#');
    configureLog("$root/child");
    swoole_error_log(SWOOLE_LOG_NOTICE, '#M14:child-private#');
    exit(0);
}
check(pcntl_waitpid($pid, $status) === $pid && pcntl_wifexited($status) && pcntl_wexitstatus($status) === 0,
    'child completes');
swoole_error_log(SWOOLE_LOG_NOTICE, '#M14:parent-after#');
preg_match_all('/#M14:([^#]+)#/', file_get_contents("$root/fork"), $matches);
check($matches[1] === ['parent-before', 'child-inherited', 'parent-after'], 'fork FILE custody/order');
check(substr_count(file_get_contents("$root/child"), '#M14:child-private#') === 1, 'child private file');
$summary['fork'] = $matches[1];
// Four inherited FILE streams append concurrently, then each process rotates.
// A pipe releases them together; record counts/order are asserted, never timing.
configureLog("$root/parallel");
$children = [];
$releases = [];
for ($writer = 0; $writer < 4; $writer++) {
    $barrier = stream_socket_pair(STREAM_PF_UNIX, STREAM_SOCK_STREAM, STREAM_IPPROTO_IP);
    check($barrier !== false, 'parallel barrier');
    $pid = pcntl_fork(); check($pid >= 0, 'parallel fork succeeds');
    if ($pid === 0) {
        foreach ($releases as $release) { fclose($release); }
        fclose($barrier[0]);
        check(fread($barrier[1], 1) === 'x', 'parallel writer released');
        fclose($barrier[1]);
        for ($record = 0; $record < 64; $record++) {
            if ($record === 32) { swoole_async_set(['log_rotation' => SWOOLE_LOG_ROTATION_DAILY]); }
            swoole_error_log(SWOOLE_LOG_NOTICE, "#M14:parallel-$writer-$record#");
        }
        exit(0);
    }
    $children[] = $pid;
    fclose($barrier[1]);
    $releases[] = $barrier[0];
}
foreach ($releases as $release) {
    check(fwrite($release, 'x') === 1, 'release parallel writer');
    fclose($release);
}
foreach ($children as $pid) {
    check(pcntl_waitpid($pid, $status) === $pid && pcntl_wifexited($status) && pcntl_wexitstatus($status) === 0,
        'parallel writer completes');
}
$rotated = glob("$root/parallel.*");
check(count($rotated) === 1, 'one shared rotated destination');
foreach (["$root/parallel", $rotated[0]] as $part => $path) {
    $lines = file($path);
    check(count($lines) === 128, 'no lost or duplicate concurrent records');
    $next = array_fill(0, 4, $part * 32);
    foreach ($lines as $line) {
        check(preg_match('/^\[M14 %[0-9]+\.-?[0-9]+\]\tNOTICE\t#M14:parallel-([0-3])-([0-9]+)#\n$/D', $line, $match) === 1,
            'concurrent record remains whole');
        $writer = (int) $match[1];
        check((int) $match[2] === $next[$writer]++, 'per-writer append order');
    }
    check($next === array_fill(0, 4, ($part + 1) * 32), 'every writer contributes all records');
}
$summary['parallel-rotation'] = ['writers' => 4, 'records' => 256, 'files' => 2];
echo json_encode($summary, JSON_THROW_ON_ERROR), "\n";
