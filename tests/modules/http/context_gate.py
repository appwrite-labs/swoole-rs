#!/usr/bin/env python3
"""Actual pinned static handler bodies versus the Rust-context StaticHandler; normal effects only."""
import argparse
from pathlib import Path
import re
import shutil
import sys
import tempfile
from gate import PIN, body, prepare_index_buffer, add_archive_arguments, prepare_archive, Sides, run
from tools.harness import ROOT, write_report


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source',type=Path,required=True)
    add_archive_arguments(parser)
    parser.add_argument('--output',default='target/reports/http-context.json')
    args=parser.parse_args();local=Path(__file__).resolve().parent
    report={'pin':PIN,'scope':__doc__,'passed':False,'full_gate_passed':False,
        'rust_instrumented':args.rust_sanitize,'rust_std_instrumented':False,
        'unrun_profiles':['previous private V2 core/reference adapter assertions','native allocation/error/partial/lifetime/reentry profiles',
            'full configured PHP/original/IO','performance','other platforms/locales']}
    def guard():
        free=shutil.disk_usage(ROOT).free
        if free<4*1024**3:
            report['blocked']={'reason':'unchanged disk guard','free_bytes':free,'guard_bytes':4*1024**3}
            write_report(args.output,report);return False
        return True
    with tempfile.TemporaryDirectory(prefix='swr-http-context-') as temporary:
        tree=Path(temporary);handler='src/server/static_handler.cc';header='include/swoole_static_handler.h'
        archive=prepare_archive(args,tree,report)
        if not archive:write_report(args.output,report);return 1
        sides=Sides(args.source,args.archive,tree,handler,'src/protocol/http.cc','src/core/base.cc')
        original=sides.original(handler);original_header=sides.original(header)
        protocol=sides.original('src/protocol/http.cc');native_base=sides.original('src/core/base.cc')
        selected=selected_header=(local/'context_facade.cc').read_text()
        prepare_index_buffer(sides)
        if not guard():write_report(args.output,report);return 1
        server='''struct Server {
 std::string root="/root";bool http_autoindex=false;long timezone_=0;
 std::shared_ptr<std::unordered_set<std::string>> locations=std::make_shared<std::unordered_set<std::string>>();
 std::shared_ptr<std::vector<std::string>> http_index_files;
 std::shared_ptr<std::vector<RewriteRule>> rewrite_rules;
 const std::string &get_document_root()const{return root;}
 bool apply_rewrite_rules(StaticHandler *);
};\n'''
        # The selected side's rules are the server state's (swr_v1_server_static_rewrite).
        candidate_server=server.replace(' std::shared_ptr<std::vector<RewriteRule>> rewrite_rules;\n','').replace(' bool apply_rewrite_rules(StaticHandler *);\n',
            ' swr_v1_server_state *state=swr_v1_server_state_new();~Server(){swr_v1_server_state_free(state);}\n swr_v1_server_state *state_handle()const{return state;}\n')
        original_class=body(original_header,'class StaticHandler')+';\n'
        typed=body(selected_header,'class StaticHandler')+';\n'
        headers='#pragma once\n#include "context_oracle.h"\n#include "swoole_rs_server_state.h"\n'
        headers+='namespace upstream_context {using swoole::starts_with; class StaticHandler;\n'+server+original_class+'}\n'
        headers+='namespace candidate_context {using swoole::starts_with; class StaticHandler;\n'+candidate_server+typed+'}\n'
        (tree/'context_candidate.h').write_text(headers)
        old_signatures=re.findall(r'^((?:bool|size_t|void|std::string) StaticHandler::[^\n]+?)\s*\{',original,re.M)
        v2_signatures=re.findall(r'^((?:(?:bool|size_t|void|int32_t|uint32_t|time_t|std::string|swr_v2_http_observation|const std::string &|std::vector<swr_v2_http_static_task>)\s*)?StaticHandler::[^\n{]+|StaticHandler::~StaticHandler\([^\n{]+)',selected,re.M)
        # The constructor's initializer spans lines; body() retains it whole.
        v2_signatures=['StaticHandler::StaticHandler(', 'StaticHandler::~StaticHandler(']+[signature for signature in v2_signatures if '::StaticHandler(' not in signature and '::~StaticHandler(' not in signature]
        report['selected_method_signatures']=v2_signatures
        assert 'core_' not in typed and 'directory_' not in typed and 'tasks_' not in typed
        report['native_context_storage_absent']=True
        report['effect_name_qualification']='Only extracted lstat calls are namespace-qualified to avoid global stat-argument ADL; production and original assertions unchanged'
        for namespace in ['upstream_context','candidate_context']:
            parts=[]
            if namespace=='upstream_context':
                parts+=[body(protocol,'static int url_htoi('),body(protocol,'size_t url_decode('),body(native_base,'std::string intersection(')]
                parts += [body(original,signature) for signature in old_signatures]
                parts += [body(original,'bool Server::apply_rewrite_rules(').replace('http_server::StaticHandler *','StaticHandler *')]
            else:
                if 'static bool conditional_file_date(' in selected:
                    parts += ['template<typename Handler>\n'+body(selected,'static bool conditional_file_date(')]
                parts += [body(selected,signature) for signature in ['static void append_directory_part(', 'static int32_t resume_range_header(', 'static int32_t resume_static_path(']]
                parts += [body(selected,signature) for signature in v2_signatures]
            extracted=tree/(namespace+'.cc')
            extracted.write_text('#include "context_candidate.h"\nnamespace '+namespace+' {\n'+'\n'.join(parts).replace('lstat(',namespace+'::lstat(')+'\n}\n')
        dependency=tree/'dependencies.cc';dependency.write_text('#include "context_oracle.h"\n'+body(native_base,'size_t sw_snprintf('))
        binary=tree/'gate'
        units=[('upstream',tree/'upstream_context.cc'),('candidate',tree/'candidate_context.cc'),('upstream',dependency),('candidate',local/'context_gate.cc')]
        if not guard() or not sides.build(binary,units,archive,report,gc_sections=True):write_report(args.output,report);return 1
        return run(binary,['swr_v2_http_context_new','swr_v2_http_context_tasks','swr_v2_http_context_path','swr_v2_http_context_scan','swr_v2_http_context_index','swr_v2_http_context_page_step','swr_v2_http_context_page_emit','swr_v2_http_context_boundary','swr_v2_http_context_serve'],args.output,report)
if __name__=='__main__':sys.exit(main())
