#include "rewrite_oracle.h"
#include "swoole_rs_http_context.h"
#include "swoole_rs_server_state.h"
using Rules=std::vector<std::pair<std::string,std::string>>;
static size_t cases=0;
// The candidate: the server's Rust-owned rules rewriting a Rust static context.
struct Candidate {
 swr_v1_server_state *state=swr_v1_server_state_new();swr_v2_http_context *context;
 explicit Candidate(const std::string &url):context(swr_v2_http_context_new(reinterpret_cast<const uint8_t *>(url.data()),url.size(),4096)){if(!state||!context)throw std::bad_alloc();}
 ~Candidate(){swr_v2_http_context_drop(context);swr_v1_server_state_free(state);}
 Candidate(const Candidate &)=delete;
 std::string text(uint32_t kind)const{size_t n=0;if(swr_v2_http_context_text(context,kind,nullptr,0,&n)<0)throw std::logic_error("text");std::string v(n,'\0');if(n&&swr_v2_http_context_text(context,kind,reinterpret_cast<uint8_t *>(&v[0]),n,&n)<0)throw std::logic_error("text");return v;}
};
// Bytes outside printable ASCII, and the backslash, as \xHH.
static std::string show(const std::string &text){std::string out;for(unsigned char c:text){if(c>=32&&c<127&&c!='\\')out+=char(c);else{char hex[5];std::snprintf(hex,sizeof hex,"\\x%02x",c);out+=hex;}}return out;}
static void compare(const std::string &url,const Rules &rules){
 upstream_rewrite::Server old;Candidate candidate(url);
 for(auto &r:rules){old.add_rewrite_rule(r.first,r.second);swr_v1_server_static_rewrite_add(candidate.state,reinterpret_cast<const uint8_t *>(r.first.data()),r.first.size(),reinterpret_cast<const uint8_t *>(r.second.data()),r.second.size());}
 upstream_rewrite::http_server::StaticHandler a(url);
 const bool x=old.apply_rewrite_rules(&a),y=swr_v1_server_static_rewrite(candidate.state,candidate.context);
 if(x!=y||a.url!=candidate.text(SWR_HTTP_CONTEXT_URL)||a.original!=candidate.text(SWR_HTTP_CONTEXT_ORIGINAL)||a.publications!=unsigned(y))throw std::runtime_error("rewrite outcome mismatch case "+std::to_string(cases)+": "+show(a.url)+" vs "+show(candidate.text(SWR_HTTP_CONTEXT_URL)));
 ++cases;
}
// Random patterns from std::regex's ECMAScript grammar, one rule each, over short URLs of
// the same bytes; every mismatch is listed. No atom starts with a digit, `=` or `!`, so none
// forms a backreference or lookahead, which the Rust dialect leaves out.
static void regex_sweep(){
 static const char *const atoms[]={"a","b","/","\x80","\xff",".","\\d","\\w","\\s","\\W","\\b","\\B","\\x80","\\xff","\\u00e9","\\cJ","\\0","\\/","\\.","[ab]","[^a]","[a-\\xff]","[\\x7f-\\x80]","[\\x80-\\xff]","[\\d-]","[[:alpha:]]","[]","[^]","^","$","(",")","(?:","|","*","+","?","*?","+?","??","{2}","{0,1}","{1,}","{0}","{","}","]","[","-",",","\\"};
 static const char *const bytes[]={"a","b","/","\x80","\xff","1"," ","\n","_"};
 static const char *const formats[]={"X","$&","[$1]","<$2>","$`","$'","","$$"};
 std::mt19937 rng(7349);unsigned failures=0;
 for(unsigned i=0;i<10000;++i){
  std::string pattern,url;for(unsigned n=1+rng()%7;n--;)pattern+=atoms[rng()%std::size(atoms)];
  for(unsigned n=rng()%9;n--;)url+=bytes[rng()%std::size(bytes)];
  try{compare(url,{{"~"+pattern+"~",formats[rng()%std::size(formats)]}});}
  catch(const std::runtime_error &e){if(++failures<=80)std::cerr<<"~"<<show(pattern)<<"~ on \""<<show(url)<<"\": "<<e.what()<<'\n';}
 }
 if(failures)throw std::runtime_error(std::to_string(failures)+" random regex rules disagree");
}
int main(){try{
 std::vector<Rules> rules={ {},{{"",""}},{{"","/fallback"}},{{"/a","/b"}},{{"/a","/a"}},{{"/a","/first"},{"/a","/second"}},{{"~^/a$~","/regex"},{"/","/literal"}},{{"~[~","invalid"},{"/a","fallback"}},{{"~a~","a"},{"","fallback"}},{{"~~","x"}},{{"~(a)(b)~","$2/$1"}},{{"~","/literal"}},{{std::string("/a\0",3),std::string("/b\0",3)}},
  // The regex dialect: every match replaced, empty matches, the $ format, classes and anchors.
  {{"~a*~","X"}},{{"~a|~","X"}},{{"~b*~","-"}},{{"~(a)|(b)~","[$1|$2]"}},{{"~a~","[$`|$&|$'|$$|$x|$9|$01|$10|$]"}},{{"~a~","$"}},
  {{"~^/view/post/(\\d+)$~","/static/$1.html"},{"/article/","/static/article/"}},{{"~\\w+~","<$&>"}},{{"~\\s~","_"}},{{"~.~","."}},{{"~\\b~","|"}},{{"~[^/]+$~","index"}},{{"~(/a)+~","$1"}},{{"~a{2}~","2"}},{{"~\\/~","\\"}},{{"~^~","^"}},{{"~$~","$"}},
  // Lazy quantifiers, alternation order, counted repeats and POSIX classes.
  {{"~a+?~","<$&>"}},{{"~(a|ab)(c|bcd)(d*)~","[$1|$2|$3]"}},{{"~x{1,2}?~","y"}},{{"~(?:ab)+~","-"}},{{"~[a-c]+~","<$&>"}},{{"~\\d{2,}~","N"}},{{"~[^a-z/]~","_"}},{{"~[[:digit:]]+~","#"}},{{"~[[:alpha:]_]+$~","$&!"}},
  // A non-empty match after an empty one, escapes as libstdc++ reads them, and the state limit.
  {{"~a*?~","X"}},{{"~|a~","X"}},{{"~\\B|^b~","X"}},{{"~\\x41\\u0142\\cJ\\q\\A\\01~","<$&>"}},{{"~[[:DIGIT:]]+~","#"}},{{"~a(?:){100000}~","b"}},{{"~a(?:a{0}){100000}~","b"}},{{"~a(?:){4294967295}~","b"}} };
 std::vector<std::string> urls={"","/","/a","/a/b","/ab","/aa",std::string("/a\0tail",7),std::string(4096,'a'),"baaac","/view/post/3","/view/post/x","/article/settings.html","a b\r\nc","/a/a/a","abcd","/a1b22c333/_x","abab",std::string("ABJqA\0" "1",7)};
 for(auto &url:urls)for(auto &r:rules)compare(url,r);
 {std::mt19937 rng(34129);for(unsigned i=0;i<2048;++i){std::string url;for(unsigned n=rng()%80;n--;)url.push_back(static_cast<char>(rng()%256));Rules r;for(unsigned n=rng()%8;n--;){std::string prefix=url.substr(0,url.empty()?0:rng()%(url.size()+1)),replacement;for(unsigned m=rng()%80;m--;)replacement.push_back(static_cast<char>(rng()%256));if(rng()%2)prefix="mismatch"+prefix;r.emplace_back(prefix,replacement);}compare(url,r);}}
 regex_sweep();
 upstream_rewrite::Server a;if(a.apply_rewrite_rules(nullptr))throw std::runtime_error("null rules short circuit");
 std::cout<<"{\"rewrite_cases\":"<<cases<<",\"errors\":0}\n";return 0;}catch(const std::exception &e){std::cerr<<e.what()<<'\n';return 1;}}
