// The selected packet cases call the exported operation over the real ABI record.
// The fixture owns only its scripted IO and String storage.
static Socket *owner(swr_v1_co_socket *s) { return reinterpret_cast<Socket *>(s->resolve_context_); }
static const swr_v1_hook_co_socket_host_t host = [] {
 swr_v1_hook_co_socket_host_t h{};
 h.current=[]()->void* {return nullptr;};
 h.timer_add=[](swr_v1_co_socket *,double)->void* {record("timer:start");return timer_ok?reinterpret_cast<void*>(1):nullptr;};
 h.timer_del=[](void*){};
 h.read_buffer=[](swr_v1_co_socket *s,swr_v1_hook_read_buffer *view,uint32_t action,size_t size) {
  auto *sock=owner(s);auto *buffer=action==0?sock->get_read_buffer():sock->read_buffer;
  if(action){buffer->length=view->length;buffer->offset=view->offset;if(action==2)buffer->extend(size);if(action==3)buffer->reduce(size);}
  *view={buffer->length,buffer->size,static_cast<int64_t>(buffer->offset),buffer->str};
 };
 h.recv=[](swr_v1_co_socket *s,char *at,size_t n,bool all){return owner(s)->receive(at,n,all);};
 h.package_length=[](swr_v1_co_socket *s,uint32_t n,uint32_t *header){
  auto *sock=owner(s);PacketLength pl{0,sock->read_buffer->str,n};
  auto length=sock->protocol.get_package_length(&sock->protocol,sock->socket,&pl);
  record("trace:"+std::to_string(length)+":"+std::to_string(n));*header=pl.header_len;return length;
 };
 h.packet_fail=[](swr_v1_co_socket *s,uint32_t kind,ssize_t length){
  auto *sock=owner(s);
  if(kind==0)sock->set_err(SW_ERROR_PACKAGE_LENGTH_NOT_FOUND,"get package length failed");
  else if(kind==1){swoole_error_log(0,SW_ERROR_PACKAGE_LENGTH_TOO_LARGE,"",sock->socket->get_addr(),sock->socket->get_port(),length);sock->set_err(SW_ERROR_PACKAGE_LENGTH_TOO_LARGE,sw_error);}
  else sock->set_err(SW_ERROR_PACKAGE_LENGTH_TOO_LARGE,"no package eof, package_max_length exceeded");
 };
 return h;
}();
ssize_t Socket::recv_packet(double timeout) {
 // Availability is exercised by the record-policy Rust tests; this facade's
 // synthetic predicate keeps the original oracle's event stream unchanged.
 if(!is_available(SW_EVENT_READ))return -1;
 record("timer:new:"+number(timeout==0?socket->read_timeout:timeout));
 swr_v1_co_socket record{};swr_v1_socket network{};
 network.read_timeout=socket->read_timeout;
 record.socket=reinterpret_cast<swoole::network::Socket*>(&network);
 record.sock_fd=100;record.state_=swr_v1_hook_socket_state_new();
 record.resolve_context_=reinterpret_cast<swoole::NameResolver::Context*>(this);
 record.open_length_check=open_length_check;record.open_eof_check=open_eof_check;record.errCode=errCode;
 auto &p=record.protocol;
 p.package_length_type=protocol.package_length_type;p.package_length_size=protocol.package_length_size;
 p.package_length_offset=protocol.package_length_offset;p.package_max_length=protocol.package_max_length;
 p.package_eof_len=protocol.package_eof_len;std::memcpy(p.package_eof,protocol.package_eof,sizeof(p.package_eof));
 swr_v1_hook_co_socket_host(&host);
 auto result=swr_v1_hook_co_socket_recv_packet(&record,timeout);
 swr_v1_hook_socket_state_free(record.state_);
 ::record("timer:destroy");
 return result;
}
