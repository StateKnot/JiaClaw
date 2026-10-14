#!/usr/bin/env python3
"""Fresh original native/gateway fixtures; real PPTX, explicit Send and reply."""
import os
from pathlib import Path
import runpy
import sys

assert len(sys.argv) in (2, 3)
mode = sys.argv[2] if len(sys.argv) == 3 else '--standalone'
assert mode in ('--standalone', '--tenant-json', '--tenant-preview')
os.environ['JIACLAW_TEST_PRESENTATION_INPUT_ONLY'] = '1'
source = 'web_stream.py' if mode == '--standalone' else 'tenant_http_browser.py'
sys.argv = [source, sys.argv[1]] + (['--preview'] if mode == '--tenant-preview' else [])
runpy.run_path(str(Path(__file__).with_name(source)), run_name='__main__')
print('ALL PRESENTATION INPUT ' + mode[2:].upper() + ' BROWSER GROUPS PASS', flush=True)
