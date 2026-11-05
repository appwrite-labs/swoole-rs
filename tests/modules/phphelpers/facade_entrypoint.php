<?php
// Ordinary direct facade consumers only; no method-alias dispatch or host fixtures.
if (!function_exists('m26_facade_custody') || !function_exists('swoole_typed_array')) {
    fwrite(STDERR,"UNSUPPORTED: actual helper and enabled stdext required\n");exit(2);
}
$events=[];
class M26FacadeStringable {
    public function __toString(): string {
        global $events;
        $events[]='convert';
        if(array_sum(swoole_typed_array('<int>',[2,3]))!==5)throw new RuntimeException('conversion reentry');
        return 'converted';
    }
}
class M26FacadeCallback {
    public function __invoke(): array {
        global $events;$events[]='invoke';
        return ['nested'=>array_sum(swoole_typed_array('<int>',[4,7]))];
    }
    public function __destruct() { global $events;$events[]='destroy'; }
}
$callback=new M26FacadeCallback();
$actual=m26_facade_custody(new M26FacadeStringable(),$callback);
unset($callback);
$expected=['copy'=>'second','self_copy'=>'second','move_source_null'=>true,'self_move_null'=>true,
    'conversion'=>'converted','retained'=>'owned','taken'=>'owned',
    'duplicates'=>[[null,null],['plain','plain'],["a","a\0B"]],'lower'=>'abc',
    'variable_copy'=>true,'variable_move'=>true,'updates'=>true,'array_count'=>4,
    'walk'=>[['key'=>'entry','value'=>'second'],['key'=>7,'value'=>'last'],['key'=>9,'value'=>'set'],['key'=>10,'value'=>'append']],
    'map'=>['entry'=>'second',7=>'last',9=>'set',10=>'append'],
    'callable_ready'=>true,'called'=>['nested'=>11],'nullable_ready'=>true,'nullable_null'=>true];
if($actual!==$expected || $events!==['convert','invoke','destroy'])throw new RuntimeException('facade result/custody mismatch: '.json_encode([$actual,$events]));
$privateEvents=[];
class M26PrivateProbe { public $scalar='initial'; public $optional=null; public $container; }
class M26PrivatePayload {
    public function __destruct() { global $privateEvents; $privateEvents[]='property:'.array_sum(swoole_typed_array('<int>',[2,3])); }
}
class M26PrivateCallback {
    public function __invoke() { return 19; }
    public function __destruct() { global $privateEvents; $privateEvents[]='callback'; }
}
$probe=new M26PrivateProbe(); $probe->container=new M26PrivatePayload();
$privateCallback=new M26PrivateCallback();
$private=m26_private_custody($probe,$privateCallback);
$privateExpected=['scalar'=>'initial','same_slot'=>true,'nullable'=>true,'nonnull'=>'ready',
    'converted_count'=>0,'reset_count'=>0,'shallow_copy'=>true,'after_copy_free'=>1,
    'persisted'=>true,'persisted_refs'=>2,'released_refs'=>1,'cache_retained'=>1,'cache_released'=>true,
    'recycled'=>'ABC','shrunk'=>'XYZ'];
if($private!==$privateExpected || $privateEvents!==['property:5','callback'])throw new RuntimeException('private helper custody mismatch: '.json_encode([$private,$privateEvents]));
unset($privateCallback);
if($privateEvents!==['property:5','callback','callback'])throw new RuntimeException('private callback retained');
// Complete common utility and registration SDK consumers.
function checkUtility(bool $condition,string $message):void { if(!$condition)throw new RuntimeException($message); }
$utilityData=(object)['a'=>'A B','bin'=>"X\0Y"];
$utility=m26_utility_custody('strlen',$utilityData);
checkUtility($utility===['callable'=>true,'name'=>'strlen','optional_name'=>true,'encoded'=>'A%00+B',
    'query'=>'a=A+B&bin=X%00Y','query_reads'=>2,'invalid_query'=>true,'cleared'=>true,
    'alias_registered'=>0,'alias_missing'=>-1,'class_alias_registered'=>0], 'complete utility/registration SDK contract');
checkUtility(m26_alias_target("N\0ame")==="N\0ame" && !function_exists('m26_alias_absent'), 'registered alias handler/binary bytes and absent origin');
$source=new ReflectionFunction('m26_alias_source');$alias=new ReflectionFunction('m26_alias_target');
checkUtility($source->getNumberOfRequiredParameters()===$alias->getNumberOfRequiredParameters()
    && (string)$source->getParameters()[0]->getType()===(string)$alias->getParameters()[0]->getType(), 'alias retains actual argument metadata');
checkUtility(class_exists('M26GuardAlias',false) && (new M26GuardAlias()) instanceof M26Guard, 'case folded persistent class alias');
class M26GuardChild extends M26Guard { public $child=1; }
$guard=new M26GuardChild();unset($guard->child);
checkUtility(!isset($guard->child), 'unset guard delegates child-only properties');
$guardError=null;
try { unset($guard->fixed); } catch (Error $error) { $guardError=$error->getMessage(); }
checkUtility($guardError==='Property fixed of class M26GuardChild cannot be unset', 'unset guard preserves root declaration and actual class diagnostic');

echo json_encode(['matches'=>count($expected),'results'=>$actual,'events'=>$events,'private'=>$private,'private_events'=>$privateEvents,'utility'=>$utility,'guard_error'=>$guardError],JSON_THROW_ON_ERROR),"\n";
