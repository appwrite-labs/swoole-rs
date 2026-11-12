<?php
// Supplemental ordinary public stdext operations; no files, ports or network.
function check(bool $ok, string $message): void {
    if (!$ok) throw new RuntimeException($message);
}
check(function_exists('swoole_typed_array'), 'UNSUPPORTED: stdext is disabled');
$summary = ['copy_mutations' => 0, 'callbacks' => 0];
$summary['callable_warnings'] = [];
foreach ([false, 0, 'M26_missing_callback'] as $callback) {
    $warnings = [];
    set_error_handler(function (int $level, string $message) use (&$warnings): bool {
        check($level === E_WARNING, 'native callable warning level');
        $nested = swoole_typed_array('<int>', [2, 3]);
        check($nested->sum() === 5, 'warning handler reenters typed helpers');
        $warnings[] = $message;
        return true;
    });
    try {
        $scheduler = new Swoole\Coroutine\Scheduler();
        $scheduler->set(['exit_condition' => $callback]);
        unset($scheduler);
    } finally {
        restore_error_handler();
    }
    check(count($warnings) === 1, 'one native constructor warning');
    check(str_contains($warnings[0], $callback ? 'is not callable' : 'illegal callback function'), 'native constructor warning text');
    $summary['callable_warnings'][] = $warnings[0];
}

for ($round = 0; $round < 16; ++$round) {
    $original = swoole_typed_array('<int>', [1, 3, 5]);
    $copy = $original;
    array_push($copy, 7);
    $copy[0] += $round;
    check($original === [1, 3, 5], 'typed list copy preserves source');
    check($copy === [1 + $round, 3, 5, 7], 'typed list copied mutation');
    check($copy->sum() === 16 + $round, 'array alias sum');
    check($copy->join(':') === implode(':', $copy), 'wrapper argument permutation');
    $original = swoole_typed_array('<string,string>', ['a' => 'first', 'b' => 'last']);
    $copy = $original;
    $copy['a'] .= '-copy';
    check($original['a'] === 'first' && $copy['a'] === 'first-copy', 'typed map copied concat');
    $summary['copy_mutations'] += 2;
}
// Ordinary caught type errors exercise the complete diagnostic projection.
// Preserve the pinned list spelling ("string>"), including its missing '<'.
$summary['nested_type_errors'] = [];
foreach ([['<string>', ['value'], 'string>'], ['<int,string>', [0 => 'value'], '<int,string>'],
          ['<string,string>', ['name' => 'value'], '<string,string>']] as [$definition, $values, $got]) {
    $outer = swoole_typed_array('<<int>>');
    $inner = swoole_typed_array($definition, $values);
    try {
        $outer[] = $inner;
        throw new RuntimeException('nested type mismatch was accepted');
    } catch (TypeError $error) {
        $expected = 'Array value type mismatch, expected `<int>`, got `' . $got . '`';
        check($error->getMessage() === $expected, 'exact native diagnostic projection');
        $summary['nested_type_errors'][] = $error->getMessage();
    }
    check($outer === [], 'failed type check preserves outer array');
}
// Complete value-policy branches through actual Zend references/classes/arrays.
class HelperExpected {}
class HelperDerived extends HelperExpected {}
class HelperOther {}
$objects = swoole_typed_array('<HelperExpected>');
$objects[] = new HelperDerived();
check($objects[0] instanceof HelperDerived, 'native instanceof accepts derived class');
$nested = swoole_typed_array('<<int>>');
$nested[] = swoole_typed_array('<int>', [1, 2]);
check($nested[0] === [1, 2], 'equal nested typed metadata accepted');
$boolean = false;
$reference =& $boolean;
$bools = swoole_typed_array('<bool>', [true]);
$bools[] = $reference;
check($bools === [true, false], 'native dereference preserves both boolean tags');
$summary['validation_matches'] = 3;
$summary['validation_errors'] = [];
$cases = [
    ['<int>', 'value', 'bad', TypeError::class, 'Array value type mismatch, expected `int`, got `string`'],
    ['<HelperExpected>', 'value', new HelperOther(), TypeError::class, 'Array value type mismatch, expected `HelperExpected`, got `HelperOther`'],
    ['<<int>>', 'value', [1, 2], TypeError::class, 'Array value type mismatch, expected `<int>`, got `array`'],
    ['<string,int>', 0, 1, TypeError::class, 'Array key type mismatch, expected `string`, got `int`'],
    ['<int>', 'named', 1, Error::class, 'Incorrect array key, must be undef or int'],
    ['<int>', 4, 1, Error::class, 'Incorrect array key `4`, out of the permitted range'],
];
foreach ($cases as [$definition, $key, $value, $exception, $message]) {
    $values = swoole_typed_array($definition);
    $caught = false;
    try {
        if ($key === 'value') $values[] = $value;
        else $values[$key] = $value;
    } catch (Throwable $error) {
        check(get_class($error) === $exception && $error->getMessage() === $message, 'exact validation error class/text');
        $summary['validation_errors'][] = $error->getMessage();
        $caught = true;
    }
    check($caught && $values === [], 'rejected validation preserves typed storage');
}
// Actual mutation publication/COW and both reference engine routes.
$numbers = swoole_typed_array('<int>', [2, 4]);
$changed = $numbers;
$append_result = ($changed[] = 7);
$value = 9;
$assignment_result = ($changed[0] = $value);
$operation_result = ($changed[1] += 3);
check($append_result === 7 && $assignment_result === 9 && $operation_result === 7, 'native assignment expression results');
check($numbers === [2, 4] && $changed === [9, 7, 7], 'native mutation COW/result publication');
$strings = swoole_typed_array('<string>', ['a']);
$string_copy = $strings;
$concat_result = ($string_copy[0] .= 'b');
check($concat_result === 'ab' && $strings === ['a'] && $string_copy === ['ab'], 'native concat expression/COW');
class HelperVMReference { public int $number = 4; }
$property = new HelperVMReference();
$references = swoole_typed_array('<int>', [&$property->number]);
$reference_copy = $references;
$typed_result = ($references[0] += 3);
check($typed_result === 7 && $property->number === 7 && $reference_copy[0] === 7, 'native typed-property reference assignment');
$shared = 4;
$ordinary = swoole_typed_array('<int>', [&$shared]);
$ordinary_result = ($ordinary[0] += 3);
$ordinary[0] += 1;
check($ordinary_result === 7 && $shared === 8 && $ordinary[0] === 8, 'native ordinary reference assignment');
$summary['mutation_matches'] = 4;
$summary['mutation_errors'] = [];
foreach (['concat', 'arithmetic'] as $operation) {
    $caught = false;
    try {
        if ($operation === 'concat') $numbers[0] .= 1;
        else $strings[0] += '1';
    } catch (TypeError $error) {
        $expected = $operation === 'concat' ? 'Only string support concat operation' : 'Only int or float support arithmetic operation';
        check($error->getMessage() === $expected, 'native operation policy diagnostic');
        $summary['mutation_errors'][] = $error->getMessage();
        $caught = true;
    }
    check($caught && $numbers === [2, 4] && $strings === ['a'], 'rejected native operation preserves values');
}
// Method frame consumers use arrays/strings and an in-memory PHP stream only.
$method_array = swoole_typed_array('<int>', [3, 4]);
$method_array_ref =& $method_array;
check($method_array_ref->sum() === 7, 'reference array method frame');
$method_string = 'AbC';
$method_string_ref =& $method_string;
check($method_string_ref->lower() === 'abc' && ('XYZ')->lower() === 'xyz', 'reference and constant string method frames');
$method_stream = fopen('php://memory', 'w+');
$method_stream_ref =& $method_stream;
check($method_stream_ref->write('memory') === 6 && $method_stream_ref->tell() === 6, 'reference resource method frame write');
check($method_stream_ref->seek(0) === 0 && $method_stream_ref->read(6) === 'memory', 'resource method frame seek/read');
check($method_stream_ref->close(), 'resource method frame close');
$summary['method_frame_matches'] = 3;
// Method invocation reenters string methods through the reordered array_map wrapper.
$invocation_values = swoole_typed_array('<int>', [10, 20, 30]);
$invocation_calls = [];
$invocation_mapped = $invocation_values->map(function (int $value) use (&$invocation_calls): int {
    $invocation_calls[] = $value;
    return ((string) $value)->length() + $value;
});
check($invocation_mapped === [12, 22, 32] && $invocation_calls === [10, 20, 30], 'nested method/reordered callback results');
check($invocation_values === [10, 20, 30], 'method invocation preserves source arguments');
$invocation_exception = null;
try {
    $invocation_values->map(function (int $value): int {
        check(('Callback')->lower() === 'callback', 'throwing callback reenters method invocation');
        throw new RuntimeException('M26 ordinary method invocation exception');
    });
} catch (RuntimeException $error) { $invocation_exception = $error->getMessage(); }
check($invocation_exception === 'M26 ordinary method invocation exception', 'native exception passes through invocation cleanup');
check($invocation_values->sum() === 60, 'method invocation usable after caught exception');
$summary['invocation_matches'] = 2;
$summary['invocation_exception'] = $invocation_exception;
// Ordinary VM facade routes: typed map deletion, reference iteration rejection,
// and unchanged mixed-array VM fallthrough. No native allocator fault injection.
$map = swoole_typed_array('<string,int>', ['keep' => 1, 'drop' => 2]);
unset($map['drop']);
check($map === ['keep' => 1] && swoole_array_is_typed($map, '<string,int>'), 'typed map deletion preserves metadata');
$total = 0;
foreach ($numbers as $number) $total += $number;
check($total === 6, 'typed list ordinary value iteration');
$plain_vm = [1, 2];
foreach ($plain_vm as &$item) $item += 1;
unset($item, $plain_vm[0]);
check($plain_vm === [1 => 3], 'ordinary untyped reference iteration/deletion falls through');
$facade_errors = [];
try { unset($numbers[0]); }
catch (Error $error) { $facade_errors[] = $error->getMessage(); }
try { foreach ($numbers as &$item) {} }
catch (Error $error) { $facade_errors[] = $error->getMessage(); }
unset($item);
check($facade_errors === ['The typed array list do not support random deletion of elements',
                        'The type array do not support using references for element value during iteration'], 'typed VM facade exact errors');
check($numbers === [2, 4], 'rejected VM operations preserve typed values');
$summary['vm_facade_matches'] = 3;
$summary['vm_facade_errors'] = $facade_errors;
// Construction/type-predicate copy, lazy-probe and cleanup outcomes.
$construction_source = swoole_typed_array('<int>', [1, 2]);
$construction_copy = swoole_typed_array('<int>', $construction_source);
$construction_copy[0] = 9;
check($construction_source === [1, 2] && $construction_copy === [9, 2], 'typed initial copy preserves native COW');
$construction_map = swoole_typed_array('<string,int>', ['first' => 1, 'last' => 2]);
check($construction_map === ['first' => 1, 'last' => 2], 'ordinary initial entries retain string keys');
$construction_nested = swoole_typed_array('<<int>>', [swoole_typed_array('<int>', [1, 2])]);
check($construction_nested === [[1, 2]], 'nested typed initial entries');
check(!swoole_array_is_typed([1], 'invalid'), 'plain predicate skips definition parse');
check(swoole_array_is_typed($construction_source) && swoole_array_is_typed($construction_source, '<int>') &&
      !swoole_array_is_typed($construction_source, '<string>'), 'typed predicates preserve optional/equal/unequal paths');
$summary['construction_matches'] = 5;
$summary['construction_errors'] = [];
try { swoole_typed_array('<string>', $construction_source); }
catch (Error $error) { $summary['construction_errors'][] = $error->getMessage(); }
try { swoole_array_is_typed($construction_source, 'invalid'); }
catch (Error $error) { $summary['construction_errors'][] = $error->getMessage(); }
check($summary['construction_errors'] === [
    'The type definition of the typed array does not match the initial values',
    "The type definition of typed array must start with '<' and end with '>'"
], 'constructor/predicate exact error cleanup outcomes');
check($construction_source->sum() === 3, 'typed initializer remains usable after rejected construction/predicate');
// Unique-target builtin routes; shared-array/native allocator lifetimes remain separate gates.
$pushed = swoole_typed_array('<int>', [1]);
$push_count = array_push($pushed, 2, 3);
check($push_count === 3 && $pushed === [1, 2, 3], 'typed push validates complete argument list');
$shifted = swoole_typed_array('<int>', [3, 4]);
$shift_count = array_unshift($shifted, 1, 2);
check($shift_count === 4 && $shifted === [1, 2, 3, 4], 'typed unshift original result/order');
$spliced = swoole_typed_array('<int>', [1, 4]);
$replacement = [2, 3];
$replacement_reference =& $replacement;
$removed = array_splice($spliced, 1, 0, $replacement_reference);
check($removed === [] && $spliced === [1, 2, 3, 4], 'typed splice array replacement');
$removed = array_splice($spliced, 2, 1, 5);
check($removed === [3] && $spliced === [1, 2, 5, 4], 'typed splice scalar replacement');
$without = swoole_typed_array('<int>', [1, 2, 3]);
$removed = array_splice($without, 1, 1);
check($removed === [2] && $without === [1, 3], 'typed splice without replacement');
$empty = swoole_typed_array('<int>', [1, 2]);
$removed = array_splice($empty, 1, 0, []);
check($removed === [] && $empty === [1, 2], 'typed splice empty replacement');
$plain = [2];
array_push($plain, 's');
array_unshift($plain, 1);
array_splice($plain, 1, 1, ['x']);
check($plain === [1, 'x', 's'], 'untyped builtin routing retains native mixed arrays');
$summary['builtin_matches'] = 7;
$summary['builtin_errors'] = [];
foreach (['push', 'unshift', 'splice-array', 'splice-scalar'] as $operation) {
    $target = swoole_typed_array('<int>', [1]);
    $caught = false;
    try {
        if ($operation === 'push') array_push($target, 2, 'bad', 3);
        elseif ($operation === 'unshift') array_unshift($target, 2, 'bad', 3);
        elseif ($operation === 'splice-array') array_splice($target, 1, 0, [2, 'bad', 3]);
        else array_splice($target, 1, 0, 'bad');
    } catch (TypeError $error) {
        check($error->getMessage() === 'Array value type mismatch, expected `int`, got `string`', 'native builtin validation diagnostic');
        $summary['builtin_errors'][] = $error->getMessage();
        $caught = true;
    }
    check($caught && $target === [1], 'builtin failure validates before original mutation');
}
// Complete definition normalization, guard and native class-string lifetime.
$normalized = swoole_typed_array(" \t< STRING , INT >\n ", ['a' => 1]);
check($normalized === ['a' => 1] && swoole_array_is_typed($normalized, '<string,int>'), 'definition lowercase/compaction publication');
$summary['definition_errors'] = [];
foreach (['', '  ', 'int', '<int', 'int>', str_repeat('a', 65535)] as $definition) {
    $caught = false;
    try { swoole_typed_array($definition); }
    catch (Error $error) {
        $expected = strlen($definition) >= 65535
            ? 'The type definition string is too long (must be less than 65535 characters)'
            : "The type definition of typed array must start with '<' and end with '>'";
        check($error->getMessage() === $expected, 'definition guard/envelope diagnostic');
        $summary['definition_errors'][] = $error->getMessage();
        $caught = true;
    }
    check($caught, 'invalid definition rejected before class lookup');
}
$definition_autoload = [];
$loader = function (string $name) use (&$definition_autoload): void {
    $definition_autoload[] = $name;
    $nested = swoole_typed_array('<int>', [2, 3]);
    check($nested->sum() === 5, 'definition autoload reenters parser');
    if ($name === 'helperdefinitionloaded') { class HelperDefinitionLoaded {} }
    else throw new RuntimeException('M26 ordinary definition autoload exception');
};
spl_autoload_register($loader);
try {
    $loaded_definition = swoole_typed_array(' < HelperDefinitionLoaded > ');
    $loaded_definition[] = new HelperDefinitionLoaded();
    check($loaded_definition[0] instanceof HelperDefinitionLoaded, 'normalized class definition lookup');
    $caught = false;
    try { swoole_typed_array('<HelperDefinitionMissing>'); }
    catch (Throwable $error) {
        $summary['definition_exception_chain'] = [];
        do {
            $summary['definition_exception_chain'][] = ['class' => get_class($error), 'message' => $error->getMessage()];
            $error = $error->getPrevious();
        } while ($error !== null);
        $caught = true;
    }
    check($caught, 'ordinary failed autoload propagates native exception chain');
} finally { spl_autoload_unregister($loader); }
check($definition_autoload === ['helperdefinitionloaded', 'helperdefinitionmissing'], 'native class lookup prefix');
$summary['definition_autoload'] = $definition_autoload;
$summary['definition_matches'] = 2;
$summary['warning_exceptions'] = [];
foreach ([false, 'M26_missing_callback'] as $callback) {
    $scheduler = null;
    set_error_handler(function (int $level): void {
        check($level === E_WARNING, 'native warning exception level');
        $nested = swoole_typed_array('<int>', [5, 7]);
        check($nested->sum() === 12, 'throwing warning handler reenters helpers');
        throw new RuntimeException('M26 ordinary warning exception');
    });
    try {
        $scheduler = new Swoole\Coroutine\Scheduler();
        $scheduler->set(['exit_condition' => $callback]);
        throw new RuntimeException('warning exception was lost');
    } catch (RuntimeException $error) {
        check($error->getMessage() === 'M26 ordinary warning exception', 'native warning exception preserved');
        $summary['warning_exceptions'][] = $error->getMessage();
    } finally {
        restore_error_handler();
        unset($scheduler);
    }
}
$source = [1, 3, 5];
$seen = [];
try {
    $source->map(function (int $value) use (&$seen): int {
        $seen[] = $value;
        $nested = swoole_typed_array('<int>', [$value, 1]);
        check($nested->sum() === $value + 1, 'callback reenters typed helpers');
        if ($value === 3) throw new RuntimeException('ordinary callback exception');
        return $value * 2;
    });
    throw new RuntimeException('callback exception was lost');
} catch (RuntimeException $error) {
    check($error->getMessage() === 'ordinary callback exception', 'callback exception preserved');
}
check($seen === [1, 3] && $source === [1, 3, 5], 'callback prefix/source preserved');
$summary['callbacks'] = count($seen);
$destructed = [];
class HelperValue {
    public function __construct(public int $id) {}
    public function __destruct() {
        global $destructed;
        $nested = swoole_typed_array('<int>', [3, 5]);
        check($nested->sum() === 8, 'destructor reenters helpers');
        $destructed[] = $this->id;
    }
}
$map = swoole_typed_array('<string,HelperValue>', ['a' => new HelperValue(1), 'b' => new HelperValue(2)]);
$copy = $map;
$map['a'] = new HelperValue(3);
check($copy['a']->id === 1 && $map['a']->id === 3, 'object typed map copy separates storage');
unset($copy);
check($destructed === [1], 'old object stays alive until final copied reference released');
unset($map);
sort($destructed);
check($destructed === [1, 2, 3], 'objects released exactly once');
$summary['destructors'] = $destructed;
$autoload = [];
spl_autoload_register(function (string $name) use (&$autoload): void {
    $autoload[] = strtolower($name);
    $nested = swoole_typed_array('<int>', [7, 9]);
    check($nested->sum() === 16, 'autoload reenters helpers');
    if (strtolower($name) === 'helperautoload') {
        class HelperAutoload {}
    }
});
$loaded = swoole_typed_array('<HelperAutoload>');
check($loaded === [] && $autoload === ['helperautoload'], 'class lookup/autoload remains native');
$summary['autoload'] = $autoload;
class HelperCycle {
    public ?HelperCycle $peer = null;
    public function __construct(public int $id) {}
    public function __destruct() {
        global $cycles;
        $nested = swoole_typed_array('<int>', [1, 2]);
        check($nested->sum() === 3, 'cycle destructor reenters helpers');
        $cycles[] = $this->id;
    }
}
$cycles = [];
$left = new HelperCycle(4); $right = new HelperCycle(5);
$left->peer = $right; $right->peer = $left;
$values = swoole_typed_array('<HelperCycle>', [$left, $right]);
unset($values, $left, $right);
gc_collect_cycles();
sort($cycles);
check($cycles === [4, 5], 'typed array releases ordinary cyclic objects');
$summary['cycle_destructors'] = $cycles;
// Additional public-API initialized-span observations; original frozen assertions above retained.
$summary['initialized_definitions'] = [];
foreach (["<in\0t>", "<string,\0int>", "<int,<str\0ing>>", "<bo\0ol,string>"] as $definition) {
    $observation = ['definition_base64' => base64_encode($definition)];
    try {
        $array = swoole_typed_array($definition);
        check($array === [] && swoole_array_is_typed($array, $definition), 'accepted initialized type metadata retained');
        $observation['accepted'] = true;
    } catch (Error $error) {
        $observation['accepted'] = false;
        $observation['exception'] = get_class($error);
        $observation['message'] = $error->getMessage();
    }
    $summary['initialized_definitions'][] = $observation;
}
// The real PCRE cache owner and version-specific match ABI, including cached
// reuse, flags, offsets and nested compilation from a warning handler.
$summary['pcre_matches']=0;
foreach ([['12 A34','/(A)?([0-9]+)/',0,0],['A12 B34 A56','/([A-Z])([0-9]+)/',PREG_OFFSET_CAPTURE,2],
          ['12 A34','/(A)?([0-9]+)/',PREG_UNMATCHED_AS_NULL,0],["A12\0A34",'/A([0-9]+)/',0,0],
          ['é12 é34','/(é)([0-9]+)/u',PREG_OFFSET_CAPTURE|PREG_UNMATCHED_AS_NULL,0],
          ['none','/A([0-9]+)/',0,0]] as [$subject,$pattern,$flags,$offset]) {
    for ($round=0;$round<32;++$round) {
        preg_match($pattern,$subject,$expected,$flags,$offset);
        check(swoole_str_match($subject,$pattern,$flags,$offset)===$expected,'cached PCRE single match flags/offset/results');
        preg_match_all($pattern,$subject,$expected,$flags|PREG_SET_ORDER,$offset);
        check(swoole_str_match_all($subject,$pattern,$flags|PREG_SET_ORDER,$offset)===$expected,'cached PCRE global match flags/offset/results');
        $summary['pcre_matches']+=2;
    }
}
$pcreWarnings=[];
set_error_handler(function (int $level,string $message) use (&$pcreWarnings):bool {
    check($level===E_WARNING,'PCRE compilation warning level');
    check(swoole_str_match('a','/a/')===['a'],'warning handler reenters real PCRE cache');
    $pcreWarnings[]=$message;return true;
});
try { check(swoole_str_match('subject','/')===false,'invalid PCRE cache result'); }
finally { restore_error_handler(); }
check(count($pcreWarnings)===1,'one original PCRE compilation warning');
$summary['pcre_warnings']=$pcreWarnings;
class HelperVMStringReference { public string $text='x'; }
$property=new HelperVMStringReference();
$references=swoole_typed_array('<string>',[&$property->text]);
$typedConcat=($references[0].='y');
check($typedConcat==='xy' && $property->text==='xy' && $references[0]==='xy','typed-property string concat reference fast path');
$summary['typed_reference_concat']=$typedConcat;
// An exit condition that replaces itself (Co::set) while the reactor runs it
// finishes on its own reference; the slot's goes with the replacement.
$exitCalls=0;
Swoole\Coroutine::set(['exit_condition'=>function () use (&$exitCalls):bool {
    ++$exitCalls;
    Swoole\Coroutine::set(['exit_condition'=>function () use (&$exitCalls):bool { ++$exitCalls; return true; }]);
    return true;
}]);
Swoole\Coroutine\run(function ():void { Swoole\Coroutine::sleep(0.01); });
check($exitCalls>0,'self-replacing exit condition ran');
$summary['exit_condition_replaced']=$exitCalls>0;
echo json_encode($summary, JSON_THROW_ON_ERROR), "\n";
