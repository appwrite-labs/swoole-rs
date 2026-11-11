"""A header or trailer value's __toString replaces the array being sent, over HTTP/2.

Each response carries the array as it was when sending began, and the connection keeps serving.
"""
import json
import os
from pathlib import Path
import subprocess

from tools.harness import server

FIXTURE = Path(__file__).parents[1] / 'http' / 'header_reentry.php'
# The client keeps the last of x-list's two fields.
HEADERS = {'x-a': 'a', 'x-b': 'b', 'x-list': 'l2'}


def test_responses_keep_the_headers_set_before_sending(engine):
    with server(engine, FIXTURE) as (port, _):
        php = subprocess.run(engine.command(Path(__file__).with_name('header_reentry_client.php')),
                             env={**os.environ, 'TEST_PORT': str(port)},
                             capture_output=True, text=True, timeout=30)
    assert php.returncode == 0, php.stdout + php.stderr
    assert [json.loads(line) for line in php.stdout.splitlines()] == [
        ['/replace-empty', 200, HEADERS, 'ok'],
        ['/replace-new', 200, HEADERS, 'ok'],
        ['/nested', 200, HEADERS, 'ok'],
        ['/trailer', 200, {'x-t1': 't1', 'x-t2': 't2'}, 'chunk'],
        ['/', 200, [], 'ok'],
        ['/client', 200, [], 'ok'],
    ], php.stdout + php.stderr
