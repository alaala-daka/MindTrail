"""Inspect only the generated NSIS script in the owned failed run."""
import json
from pathlib import Path
import sys

ROOT=Path(__file__).resolve().parent.parent
sys.path.insert(0,str(ROOT/'tests')); sys.path.insert(0,str(ROOT/'scripts'))
from release_desktop_candidate import validate_nsis_script

run=ROOT/'.tools/productization-release/run-02bb4e8bbebd45da906552912bc6cb2a'
try:
    if (run/'owner').read_text(encoding='ascii')!='02bb4e8bbebd45da906552912bc6cb2a': raise ValueError('invalid_run')
    release=run/'target/x86_64-pc-windows-msvc/release'; source=run/'source'
    digest=json.loads((run/'producer-build.json').read_text(encoding='utf8'))['sha256']
    text=(release/'nsis/x64/installer.nsi').read_text(encoding='utf8')
    validate_nsis_script(text,[source/'src-tauri/resources/producer/mindtrail-producer.exe',release/'producer/mindtrail-producer.exe'],digest,release/'mindtrail.exe')
    print('nsis_fixed_entries=matched')
except Exception: print('nsis_fixed_entries=not_matched')
