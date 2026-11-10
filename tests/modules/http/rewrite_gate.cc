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
static void compare(const std::string &url,const Rules &rules){
 upstream_rewrite::Server old;Candidate candidate(url);
 for(auto &r:rules){old.add_rewrite_rule(r.first,r.second);swr_v1_server_static_rewrite_add(candidate.state,reinterpret_cast<const uint8_t *>(r.first.data()),r.first.size(),reinterpret_cast<const uint8_t *>(r.second.data()),r.second.size());}
 upstream_rewrite::http_server::StaticHandler a(url);
 const bool x=old.apply_rewrite_rules(&a),y=swr_v1_server_static_rewrite(candidate.state,candidate.context);
 if(x!=y||a.url!=candidate.text(SWR_HTTP_CONTEXT_URL)||a.original!=candidate.text(SWR_HTTP_CONTEXT_ORIGINAL)||a.publications!=unsigned(y))throw std::runtime_error("rewrite outcome mismatch case "+std::to_string(cases)+": "+a.url+" vs "+candidate.text(SWR_HTTP_CONTEXT_URL));
 ++cases;
}
int main(){try{
 std::vector<Rules> rules={ {},{{"",""}},{{"","/fallback"}},{{"/a","/b"}},{{"/a","/a"}},{{"/a","/first"},{"/a","/second"}},{{"~^/a$~","/regex"},{"/","/literal"}},{{"~[~","invalid"},{"/a","fallback"}},{{"~a~","a"},{"","fallback"}},{{"~~","x"}},{{"~(a)(b)~","$2/$1"}},{{"~","/literal"}},{{std::string("/a\0",3),std::string("/b\0",3)}},
  // The regex dialect: every match replaced, empty matches, the $ format, classes and anchors.
  {{"~a*~","X"}},{{"~a|~","X"}},{{"~b*~","-"}},{{"~(a)|(b)~","[$1|$2]"}},{{"~a~","[$`|$&|$'|$$|$x|$9|$01|$10|$]"}},{{"~a~","$"}},
  {{"~^/view/post/(\\d+)$~","/static/$1.html"},{"/article/","/static/article/"}},{{"~\\w+~","<$&>"}},{{"~\\s~","_"}},{{"~.~","."}},{{"~\\b~","|"}},{{"~[^/]+$~","index"}},{{"~(/a)+~","$1"}},{{"~a{2}~","2"}},{{"~\\/~","\\"}},{{"~^~","^"}},{{"~$~","$"}},
  // Lazy quantifiers, alternation order, counted repeats and POSIX classes.
  {{"~a+?~","<$&>"}},{{"~(a|ab)(c|bcd)(d*)~","[$1|$2|$3]"}},{{"~x{1,2}?~","y"}},{{"~(?:ab)+~","-"}},{{"~[a-c]+~","<$&>"}},{{"~\\d{2,}~","N"}},{{"~[^a-z/]~","_"}},{{"~[[:digit:]]+~","#"}},{{"~[[:alpha:]_]+$~","$&!"}} };
 std::vector<std::string> urls={"","/","/a","/a/b","/ab","/aa",std::string("/a\0tail",7),std::string(4096,'a'),"baaac","/view/post/3","/view/post/x","/article/settings.html","a b\r\nc","/a/a/a","abcd","/a1b22c333/_x"};
 for(auto &url:urls)for(auto &r:rules)compare(url,r);
 {std::mt19937 rng(34129);for(unsigned i=0;i<2048;++i){std::string url;for(unsigned n=rng()%80;n--;)url.push_back(static_cast<char>(rng()%256));Rules r;for(unsigned n=rng()%8;n--;){std::string prefix=url.substr(0,url.empty()?0:rng()%(url.size()+1)),replacement;for(unsigned m=rng()%80;m--;)replacement.push_back(static_cast<char>(rng()%256));if(rng()%2)prefix="mismatch"+prefix;r.emplace_back(prefix,replacement);}compare(url,r);}}
 upstream_rewrite::Server a;if(a.apply_rewrite_rules(nullptr))throw std::runtime_error("null rules short circuit");
 std::cout<<"{\"rewrite_cases\":"<<cases<<",\"errors\":0}\n";return 0;}catch(const std::exception &e){std::cerr<<e.what()<<'\n';return 1;}}
