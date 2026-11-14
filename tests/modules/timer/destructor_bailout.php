<?php
[$script,$kind,$co,$mode]=$argv;
swoole_async_set(['enable_coroutine'=>(bool)(int)$co]);
class TerminalGuard {
 public function __construct(private string $mode){}
 public function __destruct(){echo "DTOR\n";if($this->mode==='exit')exit(31);if($this->mode==='oom')$blob=str_repeat('x',32*1024*1024);throw new RuntimeException('destructor failure');}
}
$id=0;register_shutdown_function(static function(){echo "SHUTDOWN\n";});
ini_set('memory_limit',(string)(memory_get_usage(true)+1024*1024));
$g=new TerminalGuard($mode);
$cb=static function()use($g,&$id,$kind){echo "CALL\n";if($kind==='tick')Swoole\Timer::clear($id);};
$id=$kind==='after'?Swoole\Timer::after(1,$cb):Swoole\Timer::tick(1,$cb);
unset($g,$cb);Swoole\Event::wait();echo "RETURN\n";
