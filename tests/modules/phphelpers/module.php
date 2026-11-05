<?php
function checkModule(bool $condition, string $message): void {
    if (!$condition) { throw new RuntimeException($message); }
}
if (($argv[1] ?? '') === 'output-exit') {
    ob_start(function (string $chunk): string { exit(37); }, 1);
    (new ReflectionExtension('swoole'))->info();
    throw new RuntimeException('information output callback did not execute');
}
$all = ini_get_all('swoole', true);
$system = ['use_shortname', 'blocking_detection', 'blocking_threshold', 'profile', 'leak_detection'];
checkModule(count($all) === 10, 'complete INI definition family');
$changes = [];
foreach ($all as $name => $entry) {
    $suffix = substr($name, strlen('swoole.'));
    $restricted = in_array($suffix, $system, true);
    checkModule($entry['access'] === ($restricted ? INI_SYSTEM : INI_ALL), 'original INI access mode');
    $old = ini_get($name);
    foreach (['0', '1', 'On', 'Off', 'true', 'false', 'yes', 'no', '12345', '-17', 'invalid'] as $value) {
        $result = @ini_set($name, $value);
        $changes[$name][] = [$value, $result, ini_get($name)];
        if ($restricted) { checkModule($result === false && ini_get($name) === $old, 'system entry rejects request mutation'); }
    }
    if (!$restricted) { ini_set($name, $old); }
}
checkModule(ini_get_all('swoole', true) === $all, 'all definitions restored after native update handlers');
ob_start();
(new ReflectionExtension('swoole'))->info();
$info = ob_get_clean();
checkModule(str_contains($info, 'Swoole => enabled') && str_contains($info, 'Author => Swoole Team <team@swoole.com>'), 'module information header and author');
checkModule(str_contains($info, 'swoole.socket_buffer_size'), 'module information includes registered INI entries');
echo json_encode(['ini' => $all, 'changes' => $changes, 'info' => $info], JSON_THROW_ON_ERROR), "\n";
