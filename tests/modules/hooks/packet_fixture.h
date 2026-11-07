static std::vector<int64_t> packet_lengths;
static size_t packet_index=0,input_index=0,read_index=0;
static std::string input;
static std::vector<ssize_t> reads;
static uint32_t next_header=4;
static bool timer_ok=true,available=true;
struct PacketLength {uint32_t header_len;char *buf;uint32_t buf_size;};
struct Protocol;
struct NativeSocket {double read_timeout=1,write_timeout=1;const char *get_addr(){record("address");return "private";}int get_port(){record("port");return 9602;}};
static ssize_t protocol_length(Protocol *,NativeSocket *,PacketLength *length){record("parse:"+std::to_string(length->buf_size));length->header_len=next_header;auto index=std::min(packet_index++,packet_lengths.size()-1);return packet_lengths.at(index);}
struct Protocol {
 uint32_t package_length_offset=0,package_length_size=2,package_max_length=16;
 char package_length_type='N';char package_eof[8]={'\r','\n'};uint32_t package_eof_len=2;
 decltype(&protocol_length) get_package_length=protocol_length;
};
struct String {
 std::vector<char> bytes;char *str;size_t size,length=0,offset=0;
 explicit String(size_t n):bytes(n),str(bytes.data()),size(n){record("buffer:new:"+std::to_string(n));}
 ~String(){record("buffer:delete");}
 void clear(){record("buffer:clear");length=offset=0;}
 void reduce(size_t n){record("buffer:reduce:"+std::to_string(n));assert(n<=length);std::memmove(str,str+n,length-n);length-=n;offset=0;}
 void extend(size_t n){record("buffer:extend:"+std::to_string(n));assert(n>=length);bytes.resize(n);str=bytes.data();size=n;}
};
static ssize_t swoole_strnpos(const char *buffer,size_t length,const char *eof,size_t eof_length){record("scan:"+std::to_string(length)+":"+std::to_string(eof_length));auto found=std::string(buffer,length).find(std::string(eof,eof_length));return found==std::string::npos?-1:static_cast<ssize_t>(found);}
static const char *sw_error="native diagnostic";
struct Socket {
 NativeSocket native;NativeSocket *socket=&native;String *read_buffer=nullptr;Protocol protocol;
 bool open_length_check=false,open_eof_check=false;int errCode=0;
 ~Socket(){delete read_buffer;}
 bool is_available(int event){record("available:"+std::to_string(event));return available;}
 void set_err(int e){record("set-error:"+std::to_string(e));errCode=errno=e;}
 void set_err(int e,const char *message){record("error:"+std::to_string(e)+":"+message);errno=e;}
 String *get_read_buffer(){record("buffer:get");if(!read_buffer)read_buffer=new String(4);return read_buffer;}
 static void timer_callback(){}
 struct TimerController {
  TimerController(void *,double timeout,Socket *,void(*)()){record("timer:new:"+number(timeout));}
  TimerController(Socket *,int,double timeout){record("timer:new:"+number(timeout));}
  ~TimerController(){record("timer:destroy");}
  bool start(){record("timer:start");return timer_ok;}
 };
 void *read_timer=nullptr,*write_timer=nullptr;
 ssize_t receive(void *buffer,size_t n,bool all){
  record(std::string(all?"recv-all:":"recv:")+std::to_string(n));
  size_t total=0;
  do {
   ssize_t requested=read_index<reads.size()?reads[read_index++]:0;
   if(requested<0){errno=-requested;return total?static_cast<ssize_t>(total):-1;}
   size_t count=std::min({static_cast<size_t>(requested),n-total,input.size()-input_index});
   if(!count)return total;
   std::memcpy(static_cast<char*>(buffer)+total,input.data()+input_index,count);input_index+=count;total+=count;
  }while(all && total<n);
  return total;
 }
 ssize_t recv(void *buffer,size_t n){return receive(buffer,n,false);}
 ssize_t recv_all(void *buffer,size_t n){return receive(buffer,n,true);}
 ssize_t recv_packet_with_length_protocol();ssize_t recv_packet_with_eof_protocol();ssize_t recv_packet(double);
};
#include ACTUAL_BODIES
