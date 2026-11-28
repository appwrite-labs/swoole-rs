<?php
// Benign collection facade behavior only. No sockets, filesystem fixtures or hooks.
$checks = 0;
function check_collection($actual, $expected, string $label): void {
    global $checks;
    if ($actual !== $expected) {
        throw new RuntimeException($label . ': ' . var_export($actual, true) . ' != ' . var_export($expected, true));
    }
    ++$checks;
}
function collection_warning(callable $operation, string $expected) {
    $warnings = [];
    set_error_handler(static function (int $level, string $message) use (&$warnings): bool {
        $warnings[] = $message;
        return true;
    });
    try { $value = $operation(); }
    finally { restore_error_handler(); }
    check_collection($warnings, [$expected], 'exact warning and active PHP method context');
    return $value;
}
final class CollectionPayload {
    public static int $destroyed = 0;
    public function __destruct() { ++self::$destroyed; }
}
final class CollectionString {
    public static int $conversions = 0;
    public function __toString(): string { ++self::$conversions; return "converted\0value"; }
}
$table = new Swoole\Table(32);
check_collection($table->column('n', Swoole\Table::TYPE_INT, 8), true, 'integer column');
check_collection($table->column('f', Swoole\Table::TYPE_FLOAT), true, 'float column');
check_collection($table->column('s', Swoole\Table::TYPE_STRING, 32), true, 'string column');
check_collection($table->create(), true, 'create');
check_collection($table->set('first', ['n' => 7, 'f' => 1.5, 's' => "a\0b"]), true, 'set');
check_collection($table->get('first', 's'), "a\0b", 'binary value');
check_collection($table->get('first', 'n'), 7, 'integer value');
check_collection($table->get('first', 'f'), 1.5, 'float value');
check_collection($table->incr('first', 'n', 2), 9, 'integer increment');
check_collection($table->decr('first', 'n'), 8, 'integer default decrement');
check_collection($table->incr('first', 'f', 0.25), 1.75, 'float increment');
check_collection($table->decr('first', 'f', 0.5), 1.25, 'float decrement');
check_collection($table->set('second', ['n' => null]), true, 'missing column initialization');
check_collection($table->get('second'), ['n' => 0, 'f' => 0.0, 's' => ''], 'clear defaults');
check_collection($table->set('first', ['n' => 11]), true, 'partial update');
check_collection($table->get('first', 's'), "a\0b", 'partial update preserves other fields');
check_collection($table->get('missing'), false, 'missing row');
check_collection($table->get('first', 'unknown'), false, 'missing field');
check_collection($table->count(), 2, 'normal count');
check_collection($table->count(1), 6, 'recursive count');
check_collection($table->count(-1), 6, 'nonzero count mode');
$keys = [];
foreach ($table as $key => $value) { $keys[] = $key; }
sort($keys);
check_collection($keys, ['first', 'second'], 'iteration');
check_collection($table->get('first', null), ['n'=>11,'f'=>1.25,'s'=>"a\0b"], 'null field selects full row');
check_collection($table->set('first', ['s'=>new CollectionString]), true, 'native string conversion');
check_collection(CollectionString::$conversions, 1, 'single native conversion effect');
check_collection($table->get('first','s'), "converted\0value", 'converted binary string');
final class CollectionCreator {
    public function __construct(private Swoole\Table $table) {}
    public function __toString(): string {
        check_collection($this->table->create(), true, 'create while column parses its name');
        return 'late';
    }
}
$reentrant = new Swoole\Table(8);
$reentrant->column('n', Swoole\Table::TYPE_INT);
check_collection(collection_warning(
    fn() => $reentrant->column(new CollectionCreator($reentrant), Swoole\Table::TYPE_INT),
    'Swoole\\Table::column(): unable to add column after table has been created'), false, 'column after its name created the table');
check_collection(collection_warning(
    fn() => $table->incr('string-error', 's'),
    "Swoole\\Table::incr(): can't execute 'incr' on a string type column"), false, 'string increment rejected');
check_collection($table->get('string-error'), ['n'=>0,'f'=>0.0,'s'=>''], 'failed string increment preserves initialized new row');
check_collection(collection_warning(
    fn() => $table->decr('string-error', 's'),
    "Swoole\\Table::decr(): can't execute 'decr' on a string type column"), false, 'string decrement rejected');
check_collection(collection_warning(
    fn() => $table->incr('missing-field', 'unknown'),
    'Swoole\\Table::incr(): column[unknown] does not exist'), false, 'unknown column rejected');
check_collection($table->exists('missing-field'), true, 'failed unknown column preserves row allocation prefix');
check_collection(collection_warning(
    fn() => $table->column('late', Swoole\Table::TYPE_INT, 8),
    'Swoole\\Table::column(): unable to add column after table has been created'), false, 'late column rejected');
check_collection(collection_warning(
    fn() => $table->column('invalid', Swoole\Table::TYPE_STRING, 0),
    'Swoole\\Table::column(): the length of string type values has to be more than zero'), false, 'string length validation precedes ready validation');
check_collection($table->del('second'), true, 'delete');
check_collection($table->exists('second'), false, 'deleted existence');
check_collection($table->destroy(), true, 'destroy');
check_collection($table->count(), 0, 'count after destroy');
// Independent Table write cases leave the earlier inventory/count assertions intact.
$writes = new Swoole\Table(16);
check_collection($writes->column('n', Swoole\Table::TYPE_INT, 8), true, 'write integer column');
check_collection($writes->column('f', Swoole\Table::TYPE_FLOAT), true, 'write float column');
check_collection($writes->column('s', Swoole\Table::TYPE_STRING, 32), true, 'write string column');
check_collection($writes->create(), true, 'write create');
check_collection($writes->set('row', ['n'=>null,'f'=>null,'s'=>null]), true, 'new null fields clear');
check_collection($writes->get('row'), ['n'=>0,'f'=>0.0,'s'=>''], 'new null field defaults');
check_collection($writes->set('row', ['s'=>"set\0value",'f'=>4.5,'n'=>12]), true, 'existing reversed-order fields');
check_collection($writes->set('row', ['n'=>null,'f'=>null,'s'=>null]), true, 'existing null fields convert');
check_collection($writes->get('row'), ['n'=>0,'f'=>0.0,'s'=>''], 'existing null conversion values');
check_collection($writes->set('row', [0=>'ignored','unknown'=>'ignored','s'=>new CollectionString]), true, 'numeric and unknown keys ignored');
check_collection(CollectionString::$conversions, 2, 'one write string conversion');
check_collection($writes->get('row'), ['n'=>0,'f'=>0.0,'s'=>"converted\0value"], 'existing ignored keys preserve fields');
check_collection($writes->set('empty', []), true, 'empty new input');
check_collection($writes->get('empty'), ['n'=>0,'f'=>0.0,'s'=>''], 'empty new input clears every field');
check_collection($writes->set('row', []), true, 'empty update input');
check_collection($writes->get('row','s'), "converted\0value", 'empty update preserves existing field');
check_collection($writes->destroy(), true, 'write destroy');
// Column collection consumer readiness: insertion order, growth and all three
// real PHP consumers, with aligned ordinary schemas and no original edge probes.
foreach ([1, 3, 17, 65] as $columnCount) {
    $columns = new Swoole\Table(16);
    $expected = $defaults = [];
    for ($i = 0; $i < $columnCount; ++$i) {
        $name = 'field_' . $i;
        $kind = $i % 3;
        $type = [Swoole\Table::TYPE_INT, Swoole\Table::TYPE_FLOAT, Swoole\Table::TYPE_STRING][$kind];
        check_collection($columns->column($name, $type, $kind === 2 ? 16 : 8), true, 'ordered schema column');
        $expected[$name] = $kind === 0 ? $i + 2 : ($kind === 1 ? $i + 0.25 : "v\0" . $i);
        $defaults[$name] = $kind === 0 ? 0 : ($kind === 1 ? 0.0 : '');
    }
    check_collection($columns->create(), true, 'ordered schema create');
    check_collection($columns->set('populated', array_reverse($expected, true)), true, 'new row reversed input');
    check_collection($columns->get('populated'), $expected, 'read uses schema insertion order');
    check_collection($columns->set('defaults', []), true, 'new empty row initializes every column');
    check_collection($columns->get('defaults'), $defaults, 'all ordered defaults');
    check_collection($columns->count(), 2, 'ordered normal row count');
    check_collection($columns->count(1), 2 * $columnCount, 'ordered recursive count');
    check_collection($columns->count(-1), 2 * $columnCount, 'ordered nonzero count');
    $last = 'field_' . ($columnCount - 1);
    check_collection($columns->set('populated', [$last => $defaults[$last]]), true, 'last-column partial update');
    $expected[$last] = $defaults[$last];
    check_collection($columns->get('populated'), $expected, 'partial update preserves other fields and order');
    $rows = [];
    foreach ($columns as $key => $row) { $rows[$key] = $row; }
    ksort($rows);
    check_collection($rows, ['defaults' => $defaults, 'populated' => $expected], 'iterator materializes all ordered columns');
    check_collection($columns->del('defaults'), true, 'ordered delete');
    check_collection($columns->count(1), $columnCount, 'recursive count after delete');
    check_collection($columns->destroy(), true, 'ordered schema destroy');
}
// Duplicate declarations remain separate ordered storage entries; lookup keeps
// the first declaration while full-row materialization overwrites the same key.
$duplicates = new Swoole\Table(16);
check_collection($duplicates->column('duplicate', Swoole\Table::TYPE_INT, 8), true, 'duplicate first declaration');
check_collection($duplicates->column('middle', Swoole\Table::TYPE_INT, 8), true, 'duplicate middle declaration');
check_collection($duplicates->column('duplicate', Swoole\Table::TYPE_INT, 8), true, 'duplicate later declaration');
check_collection($duplicates->create(), true, 'duplicate schema create');
check_collection($duplicates->set('row', ['duplicate'=>7, 'middle'=>11]), true, 'duplicate new row initializes both entries');
check_collection($duplicates->get('row'), ['duplicate'=>7, 'middle'=>11], 'duplicate initial materialization');
check_collection($duplicates->count(1), 3, 'duplicate recursive count includes every declaration');
check_collection($duplicates->set('row', ['duplicate'=>9]), true, 'duplicate existing-row lookup updates first declaration');
check_collection($duplicates->get('row', 'duplicate'), 9, 'duplicate field lookup selects first');
check_collection($duplicates->get('row'), ['duplicate'=>7, 'middle'=>11], 'duplicate full row reads later declaration');
check_collection($duplicates->set('empty', []), true, 'duplicate empty new row initializes all entries');
check_collection($duplicates->get('empty'), ['duplicate'=>0, 'middle'=>0], 'duplicate ordered clear defaults');
check_collection($duplicates->destroy(), true, 'duplicate schema destroy');
Swoole\Coroutine\run(function (): void {
    $negative = new Swoole\Coroutine\Channel(PHP_INT_MIN);
    check_collection($negative->capacity, 1, 'negative capacity normalization');
    unset($negative);
    $channel = new Swoole\Coroutine\Channel(0);
    check_collection($channel->capacity, 1, 'capacity normalization');
    check_collection($channel->isEmpty(), true, 'initially empty');
    check_collection($channel->push("x\0y", 0.01), true, 'push binary');
    check_collection($channel->errCode, SWOOLE_CHANNEL_OK, 'push OK publication');
    check_collection($channel->isFull(), true, 'full after push');
    check_collection($channel->length(), 1, 'length after push');
    check_collection($channel->stats(), ['consumer_num'=>0,'producer_num'=>0,'queue_num'=>1], 'stats');
    check_collection($channel->push('full', 0.001), false, 'bounded full push timeout');
    check_collection($channel->errCode, SWOOLE_CHANNEL_TIMEOUT, 'push timeout publication');
    check_collection($channel->pop(0.01), "x\0y", 'binary pop');
    check_collection($channel->errCode, SWOOLE_CHANNEL_OK, 'pop OK publication');
    check_collection($channel->pop(0.001), false, 'bounded empty pop timeout');
    check_collection($channel->errCode, SWOOLE_CHANNEL_TIMEOUT, 'pop timeout publication');
    check_collection($channel->push(false, 0.01), true, 'false payload accepted');
    check_collection($channel->pop(0.01), false, 'false payload transferred');
    check_collection($channel->errCode, SWOOLE_CHANNEL_OK, 'false payload distinguished by errCode');
    $payload = new CollectionPayload;
    $weak = WeakReference::create($payload);
    check_collection($channel->push($payload,0.01), true, 'object push');
    unset($payload);
    check_collection($weak->get() !== null, true, 'queue retains sole payload');
    $popped = $channel->pop(0.01);
    check_collection($weak->get() === $popped, true, 'pop transfers original object');
    unset($popped);
    check_collection($weak->get(), null, 'popped sole reference released');
    check_collection(CollectionPayload::$destroyed, 1, 'one popped destructor');
    check_collection($channel->close(), true, 'close');
    check_collection($channel->push('closed', 0.01), false, 'closed push');
    check_collection($channel->errCode, SWOOLE_CHANNEL_CLOSED, 'closed push publication');
    check_collection($channel->pop(0.01), false, 'closed empty pop');
    check_collection($channel->errCode, SWOOLE_CHANNEL_CLOSED, 'closed pop publication');
    $payload = new CollectionPayload;
    $weak = WeakReference::create($payload);
    check_collection($channel->push($payload,0.01), false, 'closed object push rejected');
    check_collection($weak->get() === $payload, true, 'failed push preserves caller custody');
    unset($payload);
    check_collection($weak->get(), null, 'failed push retains no extra reference');
    check_collection(CollectionPayload::$destroyed, 2, 'one rejected destructor');
    $pending = new Swoole\Coroutine\Channel(1);
    $payload = new CollectionPayload;
    $weak = WeakReference::create($payload);
    check_collection($pending->push($payload,0.01), true, 'pending queue payload');
    unset($payload);
    unset($pending);
    check_collection($weak->get(), null, 'ordinary channel destruction drains payload');
    check_collection(CollectionPayload::$destroyed, 3, 'one pending destructor');
});
// Successful ordinary suspension/resume paths use channel acknowledgments,
// without timing assumptions, hooks, network or shared files.
Swoole\Coroutine\run(function (): void {
    $values = new Swoole\Coroutine\Channel(1);
    $done = new Swoole\Coroutine\Channel(1);
    Swoole\Coroutine::create(function () use ($values, $done): void {
        $value = $values->pop(1);
        check_collection($values->errCode, SWOOLE_CHANNEL_OK, 'resumed consumer OK');
        check_collection($done->push($value, 1), true, 'consumer acknowledgment transfer');
    });
    check_collection($values->stats()['consumer_num'], 1, 'consumer suspended before delivery');
    $object = (object) ['binary'=>"resume\0value"];
    check_collection($values->push($object, 1), true, 'deliver to waiting consumer');
    check_collection($done->pop(1) === $object, true, 'resumed consumer preserves object identity');
    check_collection($values->stats()['consumer_num'], 0, 'consumer wait queue drained');

    $values = new Swoole\Coroutine\Channel(1);
    $done = new Swoole\Coroutine\Channel(2);
    check_collection($values->push("first\0value", 1), true, 'fill before queued producers');
    foreach ([false, ['third'=>"queued\0value"]] as $value) {
        Swoole\Coroutine::create(function () use ($values, $done, $value): void {
            check_collection($values->push($value, 1), true, 'resumed producer transfer');
            check_collection($values->errCode, SWOOLE_CHANNEL_OK, 'resumed producer OK');
            check_collection($done->push(true, 1), true, 'producer acknowledgment');
        });
    }
    check_collection($values->stats()['producer_num'], 2, 'both producers suspended before space');
    check_collection($values->pop(1), "first\0value", 'first buffered value');
    check_collection($values->pop(1), false, 'first queued producer false payload');
    check_collection($values->errCode, SWOOLE_CHANNEL_OK, 'queued false success distinguished');
    check_collection($values->pop(1), ['third'=>"queued\0value"], 'second queued producer FIFO payload');
    check_collection($done->pop(1), true, 'first producer completed');
    check_collection($done->pop(1), true, 'second producer completed');
    check_collection($values->stats(), ['consumer_num'=>0,'producer_num'=>0,'queue_num'=>0], 'all ordinary waits drained');
});
// Exact representable signed-long endpoints only: none of these operations
// enters the original undefined native overflow fallback.
$bounds = new Swoole\Table(16);
check_collection($bounds->column('n', Swoole\Table::TYPE_INT, 8), true, 'defined boundary column');
check_collection($bounds->create(), true, 'defined boundary create');
foreach ([
    ['incr', PHP_INT_MAX - 1, 1, PHP_INT_MAX],
    ['decr', PHP_INT_MIN + 1, 1, PHP_INT_MIN],
    ['incr', PHP_INT_MIN, 0, PHP_INT_MIN],
    ['decr', PHP_INT_MAX, 0, PHP_INT_MAX],
    ['incr', -7, -3, -10],
    ['decr', -7, -3, -4],
] as [$method, $initial, $amount, $expected]) {
    check_collection($bounds->set('defined', ['n'=>$initial]), true, 'defined boundary set');
    check_collection($bounds->$method('defined', 'n', $amount), $expected, 'defined boundary arithmetic return');
    check_collection($bounds->get('defined', 'n'), $expected, 'defined boundary stored scalar');
}
check_collection($bounds->destroy(), true, 'defined boundary destroy');
// Collection-only class/alias consumer checks do not invoke the mixed upstream
// serialization fixture or another subsystem.
$facade = new Swoole\Table(16);
check_collection($facade instanceof Iterator, true, 'Table implements Iterator');
check_collection($facade instanceof Countable, true, 'Table implements Countable');
check_collection($facade->column('n', Swoole\Table::TYPE_INT, 8), true, 'facade integer column');
check_collection($facade->create(), true, 'facade create');
check_collection($facade->getSize(), $facade->size, 'native size and published property agree');
check_collection($facade->getMemorySize(), $facade->memorySize, 'native memory and published property agree');
check_collection($facade->set('alias', ['n'=>31]), true, 'alias test row');
check_collection($facade->exist('alias'), true, 'exist alias');
check_collection(count($facade), 1, 'Countable dispatch');
try { $facade->__construct(16); throw new RuntimeException('duplicate constructor accepted'); }
catch (Error $error) {
    check_collection($error->getMessage(), 'Constructor of Swoole\\Table can only be called once', 'Table constructor once');
}
check_collection($facade->get('alias','n'), 31, 'constructor refusal preserves existing table');
try { serialize($facade); throw new RuntimeException('Table serialized'); }
catch (Exception $error) {
    check_collection($error->getMessage(), "Serialization of 'Swoole\\Table' is not allowed", 'Table serialization denied');
}
check_collection($facade->delete('alias'), true, 'delete alias');
check_collection($facade->exist('alias'), false, 'exist alias after delete');
check_collection($facade->destroy(), true, 'facade destroy');
$channel = new Swoole\Coroutine\Channel;
try { serialize($channel); throw new RuntimeException('Channel serialized'); }
catch (Exception $error) {
    check_collection($error->getMessage(), "Serialization of 'Swoole\\Coroutine\\Channel' is not allowed", 'Channel serialization denied');
}
unset($channel);
// Ordinary value custody uses PHP copy-on-write arrays/strings and an in-memory
// stream. No GC reentry, cancellation, external file or lifetime correction.
Swoole\Coroutine\run(function (): void {
    $channel = new Swoole\Coroutine\Channel(2);
    $original = ['nested'=>['n'=>7], 'binary'=>"copy\0value"];
    check_collection($channel->push($original, 1), true, 'array value buffered');
    $original['nested']['n'] = 11;
    $original['binary'][0] = 'X';
    $received = $channel->pop(1);
    check_collection($received, ['nested'=>['n'=>7], 'binary'=>"copy\0value"], 'buffered array isolates caller mutation');
    $received['nested']['n'] = 17;
    check_collection($original, ['nested'=>['n'=>11], 'binary'=>"Xopy\0value"], 'popped array isolates receiver mutation');
    $string = str_repeat('binary', 8) . "\0tail";
    $expected = $string;
    check_collection($channel->push($string, 1), true, 'noninterned string buffered');
    $string[0] = 'X';
    check_collection($channel->pop(1), $expected, 'buffered string isolates caller mutation');
    check_collection($channel->stats(), ['consumer_num'=>0,'producer_num'=>0,'queue_num'=>0], 'value queue drained');

    $stream = fopen('php://memory', 'w+');
    if ($stream === false) { throw new RuntimeException('in-memory stream unavailable'); }
    $identity = get_resource_id($stream);
    fwrite($stream, "stream\0value");
    rewind($stream);
    check_collection($channel->push($stream, 1), true, 'resource buffered');
    unset($stream);
    $received = $channel->pop(1);
    check_collection(is_resource($received), true, 'queue retains resource after caller release');
    check_collection(get_resource_id($received), $identity, 'resource identity transferred');
    check_collection(stream_get_contents($received), "stream\0value", 'resource contents and position preserved');
    check_collection(fclose($received), true, 'receiver owns ordinary resource close');
    unset($received);

    $done = new Swoole\Coroutine\Channel(1);
    Swoole\Coroutine::create(function () use ($channel, $done): void {
        $value = $channel->pop(1);
        check_collection($value, ['nested'=>['n'=>23]], 'suspended consumer array value');
        $value['nested']['n'] = 29;
        check_collection($done->push($value, 1), true, 'mutated consumer acknowledgment');
    });
    check_collection($channel->stats()['consumer_num'], 1, 'array consumer waiting');
    $original = ['nested'=>['n'=>23]];
    check_collection($channel->push($original, 1), true, 'array to suspended consumer');
    check_collection($done->pop(1), ['nested'=>['n'=>29]], 'consumer mutation returned');
    check_collection($original, ['nested'=>['n'=>23]], 'resumed consumer mutation preserves sender array');
    check_collection($channel->stats(), ['consumer_num'=>0,'producer_num'=>0,'queue_num'=>0], 'custody waits drained');
});
// Last, as a bailout ends the request: a conversion that bails out mid-set()
// writes neither its column nor any later one. The shutdown function reads the
// row (upstream leaves its bucket locked, so the forced unlock's log is muted).
$bailing = new Swoole\Table(8);
$bailing->column('a', Swoole\Table::TYPE_STRING, 8);
$bailing->column('b', Swoole\Table::TYPE_INT);
$bailing->column('c', Swoole\Table::TYPE_INT);
$bailing->create();
check_collection($bailing->set('k', ['a'=>'old', 'b'=>1, 'c'=>2]), true, 'row before a bailing conversion');
final class CollectionBailout {
    public function __toString(): string { trigger_error('collection bailout', E_USER_ERROR); }
}
ini_set('display_errors', '0');
swoole_async_set(['log_level' => SWOOLE_LOG_NONE]);
register_shutdown_function(static function () use ($bailing): void {
    global $checks;
    check_collection(error_get_last()['message'] ?? null, 'collection bailout', 'conversion bailed out');
    check_collection($bailing->get('k'), ['a'=>'old', 'b'=>3, 'c'=>2], 'bailing and later columns unwritten');
    echo json_encode(['checks' => $checks, 'scope' => 'bounded table/channel behavior, expected warnings, ordinary custody and one bailing conversion; no complete reentry/GC/configuration claim']), "\n";
    exit(0);
});
$bailing->set('k', ['b'=>3, 'a'=>new CollectionBailout, 'c'=>4]);
