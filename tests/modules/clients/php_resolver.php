<?php
$mode=$argv[1];
function check(bool $ok,string $message):void {if(!$ok)throw new RuntimeException($message);}
final class WitnessState {public static array $events=[];public static array $weak=[];public static ?WeakReference $context=null;}
final class WitnessCluster {
    public function __construct(private string $id,private array $nodes,private string $mode) {WitnessState::$weak[]=WeakReference::create($this);}
    public function count() {WitnessState::$events[]='count:'.$this->id.':'.count($this->nodes);if($this->mode==='count-throw')throw new RuntimeException('count-stop');return count($this->nodes);}
    public function pop() {
        WitnessState::$events[]='pop:'.$this->id;
        if($this->mode==='pop-throw')throw new RuntimeException('pop-stop');
        if($this->mode==='bailout'){echo "native-effect-bailout\n";exit(43);}
        if($this->mode==='mutate-port')WitnessState::$context->get()->__construct(AF_INET,false);
        return array_shift($this->nodes);
    }
    public function __destruct(){WitnessState::$events[]='dtor:'.$this->id;}
}
final class WitnessResolver extends Swoole\NameResolver {
    private int $calls=0;
    public function __construct(private string $mode){}
    public function join(string $name,string $ip,int $port,array $options=[]):bool{return true;}
    public function leave(string $name,string $ip,int $port):bool{return true;}
    public function getCluster(string $name):?Swoole\NameResolver\Cluster{return null;}
    public function lookup(string $name) {
        WitnessState::$events[]='lookup:'.bin2hex($name);++$this->calls;
        if($this->mode==='lookup-throw')throw new RuntimeException('lookup-stop');
        if($this->mode==='string')return "final\0node";
        $nodes=match($this->mode){
            'cycle'=>$this->calls===1?[['host'=>"one\0node",'port'=>-19],['host'=>'two','port'=>'00042']]:[['host'=>'three','port'=>0]],
            'missing-port'=>[['host'=>'ignored']],
            default=>[['host'=>"raw\0node",'port'=>-19],['host'=>'tail','port'=>7]],
        };
        return new WitnessCluster((string)$this->calls,$nodes,$this->mode);
    }
}
$warnings=[];set_error_handler(static function(int $severity,string $message)use(&$warnings):bool{$warnings[]=[$severity,$message];return true;});
$resolver=new WitnessResolver($mode);check(swoole_name_resolver_add($resolver),'register');
$ctx=new Swoole\NameResolver\Context(AF_INET,true);$weak=WeakReference::create($ctx);WitnessState::$context=$weak;$results=[];$exception=null;
try {
    $count=$mode==='cycle'?3:($mode==='count-throw'?2:1);
    for($i=0;$i<$count;++$i)$results[]=bin2hex(swoole_name_resolver_lookup('127.0.0.1',$ctx));
}catch(Throwable $error){$exception=$error->getMessage();$error=null;}
if(str_ends_with($mode,'-throw'))check($exception===str_replace('-throw','-stop',$mode),'exception preserved');
else {
    check($exception===null,'unexpected exception:'.$exception);
    $expected=match($mode){'string'=>["final\0node"],'cycle'=>["one\0node:-19",'two:42','three:0'],'mutate-port'=>["raw\0node"],'missing-port'=>['127.0.0.1'],default=>["raw\0node:-19"]};
    check($results===array_map('bin2hex',$expected),'exact results:'.json_encode($results));
}
$ctx=null;gc_collect_cycles();check($weak->get()===null,'context collected');foreach(WitnessState::$weak as $item)check($item->get()===null,'cluster collected');
check($warnings===[],'warnings:'.json_encode($warnings));
if($mode==='cycle')check(WitnessState::$events===['lookup:3132372e302e302e31','pop:1','count:1:1','pop:1','count:1:0','dtor:1','lookup:3132372e302e302e31','pop:2','dtor:2'],'exact count/retry/destructor ordering');
echo json_encode(['mode'=>$mode,'results_hex'=>$results,'exception'=>$exception,'events'=>WitnessState::$events,'context_and_clusters_collected'=>true,'warnings'=>$warnings],JSON_THROW_ON_ERROR),"\n";
