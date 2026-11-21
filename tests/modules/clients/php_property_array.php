<?php
// An array property that holds another value is replaced, and the old value's
// __destruct may write that property again: the client keeps what it wrote.
final class Rewrite
{
    public function __construct(private Closure $write) {}
    public function __destruct() { ($this->write)(); }
}

$client = new Swoole\Coroutine\Http\Client('127.0.0.1', 80);
$client->uploadFiles = new Rewrite(function () use ($client) { $client->uploadFiles = ['kept']; });
$array = $client->addData('one', 'a');
$uploads = array_map(fn($entry) => is_array($entry) ? $entry['name'] : $entry, $client->uploadFiles);

$client->uploadFiles = new Rewrite(function () use ($client) { $client->uploadFiles = 'text'; });
$scalar = $client->addData('two', 'b');

$client->setting = new Rewrite(function () use ($client) { $client->setting = ['timeout' => 1]; });
$set = $client->set(['keep_alive' => false]);

echo json_encode([
    'array' => [$array, $uploads],
    'scalar' => [$scalar, $client->uploadFiles],
    'set' => [$set, $client->setting],
], JSON_THROW_ON_ERROR), "\n";
