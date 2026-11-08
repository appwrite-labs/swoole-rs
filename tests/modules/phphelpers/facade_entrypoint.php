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
$expected=['callable_ready'=>true,'called'=>['nested'=>11]];
if($actual!==$expected || $events!==['invoke','destroy'])throw new RuntimeException('facade result/custody mismatch: '.json_encode([$actual,$events]));
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
$privateExpected=['converted_count'=>0,'shallow_copy'=>true];
if($private!==$privateExpected || $privateEvents!==['property:5'])throw new RuntimeException('private helper custody mismatch: '.json_encode([$private,$privateEvents]));
unset($privateCallback);
if($privateEvents!==['property:5','callback'])throw new RuntimeException('private callback retained');
// Complete common utility and registration SDK consumers.
function checkUtility(bool $condition,string $message):void { if(!$condition)throw new RuntimeException($message); }
$utilityData=(object)['a'=>'A B','bin'=>"X\0Y"];
$utility=m26_utility_custody('strlen',$utilityData);
checkUtility($utility===['class_alias_registered'=>0], 'complete utility/registration SDK contract');
checkUtility(class_exists('M26GuardAlias',false) && (new M26GuardAlias()) instanceof M26Guard, 'case folded persistent class alias');

echo json_encode(['matches'=>count($expected),'results'=>$actual,'events'=>$events,'private'=>$private,'private_events'=>$privateEvents,'utility'=>$utility],JSON_THROW_ON_ERROR),"\n";
