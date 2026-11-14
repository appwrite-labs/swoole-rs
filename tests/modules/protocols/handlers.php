<?php
declare(strict_types=1);
use Swoole\Redis\Server;

$events = [];
set_error_handler(static function (int $severity, string $message) use (&$events): bool {
    $events[] = [$severity, base64_encode($message)];
    return true;
});
$server = new Server('127.0.0.1', 0, SWOOLE_BASE);
if ($argc > 1) {
    $server->setHandler(str_repeat('x', (int)$argv[1]), static function (): void {});
    echo "unexpected-valid\n";
    exit(2);
}
$registered = ['ECHO', "ECHO\0ignored", "\0leading", 'MiXeD', str_repeat('a', 53),
    str_repeat('a', 54), str_repeat('a', 54).'B', str_repeat('a', 54).'C'.str_repeat('z', 8),
    "\xffUPPER", "\x80lower"];
foreach ($registered as $index => $command) {
    $server->setHandler($command, static function () use ($index): int { return $index; });
}
$queries = [...$registered, '', 'missing', 'echo', "echo\0other", 'mixed',
    str_repeat('a', 54).'different'.str_repeat('z', 200), str_repeat('a', 53).'b'];
for ($byte = 0; $byte < 256; ++$byte) {
    $queries[] = chr($byte).'UPPER';
    $queries[] = 'MiXeD'.chr($byte).'tail';
}
$results = [];
foreach ($queries as $command) {
    $callback = $server->getHandler($command);
    $results[] = [base64_encode($command), is_callable($callback) ? $callback() : get_debug_type($callback)];
}
$properties = [];
foreach (get_object_vars($server) as $name => $value) {
    if (str_starts_with($name, '_handler_')) {
        $properties[] = [base64_encode($name), $value()];
    }
}
echo json_encode(['lookups'=>$results, 'properties'=>$properties, 'events'=>$events], JSON_THROW_ON_ERROR), "\n";
