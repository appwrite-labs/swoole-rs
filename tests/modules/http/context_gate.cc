#include "context_candidate.h"
#include <cerrno>
static void check(bool value,const char *message){if(!value)throw std::runtime_error(message);}
struct Snapshot {
 std::vector<swr_v2_http_static_task> tasks;
 std::vector<std::string> trace;
 std::string type,end,filename,date;
 size_t length=0;int status=0,error=0;bool mime_identity=false,content_identity=false,modified=false;
};
static void install_rules(upstream_context::Server &server,std::vector<upstream_context::RewriteRule> rules) {
 server.rewrite_rules=std::make_shared<std::vector<upstream_context::RewriteRule>>(std::move(rules));
}
// The selected server's rules are Rust's: a regex rule goes in between its `~` delimiters.
static void install_rules(candidate_context::Server &server,std::vector<upstream_context::RewriteRule> rules) {
 for(const auto &rule:rules){const std::string pattern=rule.is_regex?"~"+rule.pattern+"~":rule.pattern;
  swr_v1_server_static_rewrite_add(server.state,reinterpret_cast<const uint8_t *>(pattern.data()),pattern.size(),reinterpret_cast<const uint8_t *>(rule.replacement.data()),rule.replacement.size());}
}
template<typename Server>static void configure_location(Server &server){server.locations->insert("/");}
template<typename Server>static void configure_indexes(Server &server,const std::vector<std::string> &names){server.http_index_files=std::make_shared<std::vector<std::string>>(names);}
template<typename Server>static void configure_rules(Server &server,unsigned mode){
 if(!mode)return;
 using Rule=upstream_context::RewriteRule;
 std::vector<Rule> rules;
 switch(mode){
 case 1:rules.push_back({"/file","/normal",false});break;
 case 2:rules.push_back({"^/file","/normal",true});break;
 case 3:rules.push_back({"/missing","/skip",false});rules.push_back({"/file","/normal",false});break;
 case 4:rules.push_back({"^/missing","/skip",true});rules.push_back({"^/file","/normal",true});break;
 case 5:rules.push_back({"^/file","/file",true});rules.push_back({"/file","/normal",false});break;
 case 6:rules.push_back({"/file","/file",false});rules.push_back({"^/file","/should-not-apply",true});break;
 default:throw std::logic_error("unknown normal rule mode");
 }
 install_rules(server,std::move(rules));
}
static Snapshot old_file(size_t size,const std::string &url,const char *range,const char *date,const std::string &mime,unsigned repeat,unsigned rewrite,bool locations=false){
 using namespace upstream_context;
 file_size=size;upstream_context::mime=mime;directory_name.clear();link_name=url=="/alias.txt"?"/root/alias.txt":"";canonical_name=link_name.empty()?"":"/root/file.txt";trace.clear();
 Server server;
 if(locations)configure_location(server);
 configure_rules(server,rewrite);
 StaticHandler handler(&server,url.data(),url.size());
 check(handler.try_serve(),"original normal file selection");
 check(handler.try_serve(),"original repeated normal file selection");
 errno=EINTR;handler.parse_range(range,date);
 if(repeat)handler.parse_range(repeat==1?"bytes=0-0":repeat==2?nullptr:"",nullptr);
 Snapshot out;
 for(const auto &task:handler.get_tasks()){swr_v2_http_static_task copy{};copy.offset=task.offset;copy.length=task.length;memcpy(copy.part_header,task.part_header,256);out.tasks.push_back(copy);}
 const auto &type=handler.get_content_type();out.content_identity=out.tasks.size()>1?&type!=&upstream_context::mime:&type==&upstream_context::mime;out.type=type;out.modified=handler.is_modified("Tue, 14 Nov 2023 22:13:20 GMT");out.end=handler.get_end_part();out.filename=handler.get_filename();out.date=handler.get_date_last_modified();out.length=handler.get_content_length();out.status=handler.status_code;out.mime_identity=&handler.get_mimetype()==&upstream_context::mime;out.error=errno;out.trace=trace;
 return out;
}
static Snapshot selected_file(size_t size,const std::string &url,const char *range,const char *date,const std::string &mime,unsigned repeat,unsigned rewrite,bool locations=false){
 using namespace candidate_context;
 file_size=size;candidate_context::mime=mime;directory_name.clear();link_name=url=="/alias.txt"?"/root/alias.txt":"";canonical_name=link_name.empty()?"":"/root/file.txt";trace.clear();
 Server server;
 if(locations)configure_location(server);
 // Replace this field after construction to verify the selected facade retained
 // the typed field view, rather than an early shared_ptr/configuration snapshot.
 Snapshot out;
 {StaticHandler handler(&server,url.data(),url.size());
 configure_rules(server,rewrite);
 check(handler.try_serve(),"selected normal file selection");
 check(handler.try_serve(),"selected repeated normal file selection");
 errno=EINTR;handler.parse_range(range,date);
 if(repeat)handler.parse_range(repeat==1?"bytes=0-0":repeat==2?nullptr:"",nullptr);
 out.tasks=handler.copy_tasks();std::string scratch;const auto &type=handler.get_content_type(scratch);out.content_identity=&type==&scratch;out.type=type;out.modified=handler.is_modified("Tue, 14 Nov 2023 22:13:20 GMT");
 char end[64],filename[PATH_MAX];size_t count=handler.copy_end_part(end,sizeof(end));out.end.assign(end,count);handler.copy_filename(filename,sizeof(filename));out.filename=filename;
 out.date=handler.get_date_last_modified();out.length=handler.get_content_length();out.status=handler.get_status();out.mime_identity=handler.get_mimetype()==candidate_context::mime;out.error=errno;out.trace=trace;}
 for(const auto &task:out.tasks)check(memchr(task.part_header,0,256)!=nullptr,"copied task survives complete context destruction");
 return out;
}
static void check_snapshot(const Snapshot &old,const Snapshot &selected){
  check(old.tasks.size()==selected.tasks.size(),"complete context task count");
  for(size_t i=0;i<old.tasks.size();++i)check(old.tasks[i].offset==selected.tasks[i].offset&&old.tasks[i].length==selected.tasks[i].length&&memcmp(old.tasks[i].part_header,selected.tasks[i].part_header,256)==0,"complete initialized task records");
  check(old.type==selected.type&&old.end==selected.end&&old.filename==selected.filename&&old.date==selected.date&&old.length==selected.length&&old.status==selected.status,"complete context text/accounting/status/date");
  check(old.mime_identity&&selected.mime_identity&&old.content_identity&&selected.content_identity,"the original global MIME reference, the candidate MIME copy, and scoped content references");
  check(old.modified==selected.modified,"context conditional-date observation");
  check(old.error==selected.error&&old.trace==selected.trace,"normal native external effects and errno");
}
int main(){try{
 size_t files=0,directories=0;
 for(size_t size:{size_t(0),size_t(1),size_t(17),size_t(4096)})
 for(const std::string &url:{std::string("/file.txt"),std::string("/file%20name.txt?version=1")})
 for(const char *range:{static_cast<const char *>(nullptr),"","bytes=0-0","bytes=0-","bytes=-1","bytes=0-0,1-2","bytes=1-3,5-7"})
 for(const char *date:{static_cast<const char *>(nullptr),"Tue, 14 Nov 2023 22:13:20 GMT","Wed, 15 Nov 2023 22:13:20 GMT"})
 for(const std::string &mime:{std::string("text/plain"),std::string("application/octet-stream")})
 for(unsigned repeat:{0u,1u,2u,3u})for(unsigned rewrite:{0u,1u,2u,3u,4u,5u,6u}){
  const auto old=old_file(size,url,range,date,mime,repeat,rewrite);
  const auto selected=selected_file(size,url,range,date,mime,repeat,rewrite);
  check_snapshot(old,selected);
  ++files;
 }
 // Ordinary within-root symlink and configured-location anchors. Existing
 // complete file/range cross-product tuples and assertions above are unchanged.
 for(const std::string &url:{std::string("/file.txt"),std::string("/alias.txt")})
 for(bool locations:{false,true})for(size_t size:{size_t(1),size_t(17)}) {
  const auto old=old_file(size,url,nullptr,nullptr,"text/plain",0,0,locations);
  const auto selected=selected_file(size,url,nullptr,nullptr,"text/plain",0,0,locations);
  check_snapshot(old,selected);++files;
 }
 std::vector<std::vector<std::string>> listings={{".",".."},{"z.txt",".","a.txt","..","z.txt"},{"index.html","README.md",".","..","a b","a&b","<name>","é.txt"},{std::string(200,'x'),"a.txt",".",".."}};
 for(unsigned count:{1u,17u,65u,257u,1024u}){std::vector<std::string> names={".",".."};for(unsigned i=count;i--;)names.emplace_back("file-"+std::to_string(i)+".txt");names.push_back(names.back());listings.push_back(std::move(names));}
 for(const auto &entries:listings)for(const std::string &path:{std::string("/"),std::string("/normal"),std::string("/normal/")})for(unsigned mode=0;mode<6;++mode)for(size_t capacity:{16u,128u,32768u}){
  upstream_context::link_name.clear();candidate_context::link_name.clear();upstream_context::canonical_name.clear();candidate_context::canonical_name.clear();
  upstream_context::names=candidate_context::names=entries;upstream_context::trace.clear();candidate_context::trace.clear();
  upstream_context::directory_name=candidate_context::directory_name="/root"+path;
  upstream_context::Server a;candidate_context::Server b;a.http_autoindex=b.http_autoindex=mode!=5;
  if(mode){const auto configured=mode==4?std::vector<std::string>{}:mode==1||mode==5?std::vector<std::string>{"missing"}:mode==2?std::vector<std::string>{"README.md","index.html","a.txt"}:std::vector<std::string>{"a.txt","index.html","README.md"};configure_indexes(a,configured);configure_indexes(b,configured);}
  upstream_context::StaticHandler old(&a,path.data(),path.size());candidate_context::StaticHandler selected(&b,path.data(),path.size());
  check(old.try_serve()==selected.try_serve(),"directory context selection");
  check(old.try_serve_index_file()==selected.try_serve_index_file(),"real index path selection");
  check(old.get_dir_files()==selected.get_dir_files(),"directory context scan");
  check(old.get_dir_files()==selected.get_dir_files(),"directory context cache");
  upstream_context::IndexBuffer left(capacity);candidate_context::IndexBuffer right(capacity);check(old.make_index_page(&left)==selected.make_index_page(&right),"directory context page length");
  check(left.storage==right.storage&&left.length==right.length&&left.size==right.size&&left.offset==right.offset&&left.events==right.events,"directory page bytes/native buffer effects");
  check(old.make_index_page(&left)==selected.make_index_page(&right),"repeated directory context page length");
  check(left.storage==right.storage&&left.length==right.length&&left.size==right.size&&left.offset==right.offset&&left.events==right.events,"repeated directory page bytes/native buffer effects");
  char filename[PATH_MAX];selected.copy_filename(filename,sizeof(filename));check(old.get_filename()==std::string(filename),"selected complete filename backing");
  check(upstream_context::trace==candidate_context::trace,"complete directory external effect ordering");++directories;
 }
 std::cout<<"{\"normal_file_context_cases\":"<<files<<",\"normal_directory_context_cases\":"<<directories<<",\"errors\":0,\"normal_controls_only\":true}\n";
 return 0;
}catch(const std::exception &error){std::cerr<<error.what()<<'\n';return 1;}}
