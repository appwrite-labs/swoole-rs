#include "dispatch_oracle.h"
#include "allocation_oracle.h"
#include <iostream>
#include <sstream>
void check(bool yes,const char *message){if(!yes)throw std::runtime_error(message);}
int main(){
    unsigned proxies=0,proxy_faults=0,candidate_faults=0,auth_faults=0,cache_faults=0;
    std::ostringstream traces;
    try {
        const std::string long_name(200,'x');
        for(const std::string &host:{std::string(),std::string("host"),long_name})for(const std::string &user:{std::string(),std::string("u\0tail",6),long_name})for(const std::string &pwd:{std::string(),std::string("p\0tail",6),long_name}){
            const bool authenticated=!user.empty()&&!pwd.empty();
            const std::string expected_user=authenticated?user:std::string{};
            const std::string expected_password=authenticated?pwd:std::string{};
            size_t total=0,sizes[128]={};
            for(size_t nth=0;nth<=total;++nth){
                upstream_dispatch::HttpProxy::last_created=nullptr;
                bool failed=false;
                allocation::begin(nth);
                try{upstream_dispatch::HttpProxy::create(host,-123,user,pwd);}catch(const std::bad_alloc&){failed=true;}
                allocation::end();
                auto *a=upstream_dispatch::HttpProxy::last_created;
                check(allocation::count<=128,"native proxy trace overflow");
                if(!nth){
                    total=allocation::count;std::copy(allocation::sizes,allocation::sizes+total,sizes);
                    check(!failed&&a&&a->host==host&&a->port==-123&&a->username==expected_user&&a->password==expected_password,"native factory fields");
                }else{
                    check(failed&&allocation::count==nth&&std::equal(sizes,sizes+nth,allocation::sizes),"native proxy fault event prefix");
                    check(bool(a)==(nth>1),"native proxy constructor custody");
                    if(a){
                        check((a->host.empty()||a->host==host)&&(a->port==0||a->port==-123),"native partial host/port");
                        check((a->username.empty()||a->username==expected_user)&&(a->password.empty()||a->password==expected_password),"native partial credentials");
                        if(a->host!=host)check(a->port==0&&a->username.empty()&&a->password.empty(),"native host assignment precedes other fields");
                        if(!a->username.empty())check(a->host==host&&a->port==-123,"native credentials follow host/port");
                        if(!a->password.empty())check(a->username==expected_user,"native password follows username");
                    }
                    ++proxy_faults;
                }
                // The pinned control's unpublished partial object remains visible
                // to this fixture and is explicitly disposed after inspection.
                delete a;
            }
            if(proxies)traces<<',';
            traces<<"{\"native_sizes\":[";
            for(size_t i=0;i<total;++i){if(i)traces<<',';traces<<sizes[i];}
            traces<<"],\"native_faults\":"<<total;
            total=0;
            for(size_t nth=0;nth<=total;++nth){
                candidate_dispatch::HttpProxy *b=nullptr;bool failed=false;
                allocation::begin(nth);
                try{b=candidate_dispatch::HttpProxy::create(host,-123,user,pwd);}catch(const std::bad_alloc&){failed=true;}
                allocation::end();
                check(allocation::count<=128,"candidate factory trace overflow");
                if(!nth){
                    total=allocation::count;
                    check(!failed&&b&&b->host==host&&b->port==-123&&b->username==expected_user&&b->password==expected_password,"real candidate factory fields");
                }else{check(failed&&!b&&allocation::count==nth,"candidate factory C++ fault/cleanup");++candidate_faults;}
                delete b;
            }
            traces<<",\"candidate_cpp_faults\":"<<total<<'}';
            std::unique_ptr<candidate_dispatch::HttpProxy> current(candidate_dispatch::HttpProxy::create(host,-123,user,pwd));
            std::unique_ptr<upstream_dispatch::HttpProxy> control(upstream_dispatch::HttpProxy::create(host,-123,user,pwd));
            errno=EINTR;const auto expected_auth=control->get_auth_str();const int expected_errno=errno;
            errno=EINTR;const auto actual_auth=current->get_auth_str();
            check(actual_auth==expected_auth&&errno==expected_errno,"actual owner authorization/errno matches pinned control");
            total=0;
            for(size_t nth=0;nth<=total;++nth){
                bool failed=false;std::string auth;
                allocation::begin(nth);
                try{auth=current->get_auth_str();}catch(const std::bad_alloc&){failed=true;}
                allocation::end();
                if(!nth){total=allocation::count;check(!failed&&auth==expected_auth,"candidate authorization result");}
                else{check(failed&&allocation::count==nth,"candidate authorization C++ fault");++auth_faults;}
                check(current->host==host&&current->username==expected_user&&current->password==expected_password,"authorization fault preserves owner fields");
            }
            // Exercise the supported native-cache failure domain after Rust has
            // prepared replacement bytes. The real owner must retain both views
            // when the caught C++ cache allocation refuses publication.
            const std::string replacement(600,'z');
            total=0;
            for(size_t nth=0;nth<=total;++nth){
                std::unique_ptr<candidate_dispatch::HttpProxy> proxy(candidate_dispatch::HttpProxy::create(host,-123,user,pwd));
                const std::string &cache=proxy->host;const auto *cache_address=&cache;
                bool failed=false;
                allocation::begin(nth);
                try{proxy->host=replacement;}catch(const std::bad_alloc&){failed=true;}
                allocation::end();
                if(!nth){total=allocation::count;check(!failed&&proxy->host==replacement&&cache==replacement,"candidate cache publication");}
                else{check(failed&&allocation::count==nth&&proxy->host==host&&cache==host,"candidate cache refusal preserves Rust/native values");++cache_faults;}
                check(&static_cast<const std::string &>(proxy->host)==cache_address,"candidate cache reference stays stable");
                check(proxy->username==expected_user&&proxy->password==expected_password,"candidate cache fault preserves other fields");
            }
            ++proxies;
        }
        std::cout<<"{\"proxy_cases\":"<<proxies
                 <<",\"native_proxy_allocation_faults\":"<<proxy_faults<<",\"candidate_cpp_factory_faults\":"<<candidate_faults
                 <<",\"candidate_cpp_auth_faults\":"<<auth_faults<<",\"candidate_cpp_cache_faults\":"<<cache_faults<<",\"allocation_traces\":["<<traces.str()<<"],\"errors\":0}\n";
    }catch(const std::exception &e){allocation::end();std::cerr<<e.what()<<'\n';return 1;}
}
