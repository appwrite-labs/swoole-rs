<?php
// enableSSL() on a Client made without SWOOLE_SSL: the ssl_* settings given to set()
// before connect() decide the handshake. A self-signed peer fails verify_peer unless
// ssl_cafile trusts it, and ssl_host_name must match its name.
function check(bool $condition,string $message):void { if(!$condition)throw new RuntimeException($message); }
$warnings=[];set_error_handler(static function(int $severity,string $message)use(&$warnings):bool{$warnings[]=[$severity,$message];return true;});
[,$port,$cafile]=$argv;$port=(int)$port;
$cases=['verify'=>['ssl_verify_peer'=>true],
        'trusted'=>['ssl_verify_peer'=>true,'ssl_cafile'=>$cafile,'ssl_host_name'=>'localhost'],
        'wrong_host'=>['ssl_verify_peer'=>true,'ssl_cafile'=>$cafile,'ssl_host_name'=>'other.invalid']];
$sync=[];
foreach($cases as $name=>$settings){
    $client=new Swoole\Client(SWOOLE_SOCK_TCP);$client->set($settings);
    check($client->connect('127.0.0.1',$port,3),"sync $name connect");
    $sync[$name]=$client->enableSSL();
    if($sync[$name]){check($client->send("ping")===4&&$client->recv()==='ping',"sync $name echo");}
    $client->close();
}
$async=[];
foreach($cases as $name=>$settings){
    $async[$name]=false;
    $client=new Swoole\Async\Client(SWOOLE_SOCK_TCP);$client->set($settings);
    $client->on('connect',static function($client)use($name,&$async):void{
        check($client->enableSSL(static function($client)use($name,&$async):void{$async[$name]=true;$client->close();}),"async $name enableSSL");
    });
    $client->on('receive',static function():void{});
    $client->on('error',static function():void{});
    $client->on('close',static function():void{});
    check($client->connect('127.0.0.1',$port,3),"async $name connect");
    Swoole\Event::wait();
}
echo json_encode(['sync'=>$sync,'async_'=>$async,'warnings'=>$warnings],JSON_THROW_ON_ERROR),"\n";
