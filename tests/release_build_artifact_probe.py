"""Fixed-count inspection of an owned Cargo build directory."""
from pathlib import Path

ROOT=Path(__file__).resolve().parent.parent
run=ROOT/'.tools/productization-release/run-162b69a414564cbc9d0ffa7a0cec826e'
if (run/'owner').read_text(encoding='ascii')!='162b69a414564cbc9d0ffa7a0cec826e': raise SystemExit(1)
target=run/'target'; roots=(target/'release/build',target/'x86_64-pc-windows-msvc/release/build')
for name in ('tauri','tauri-build','tauri-utils','wry','ring','libsqlite3-sys','webview2-com-sys','windows','mindtrail','serde','proc-macro2'):
    dirs=[p for root in roots if root.is_dir() for p in root.iterdir() if p.is_dir() and p.name.startswith(name+'-')]
    print(name+':dirs='+str(len(dirs))+',output='+str(sum((p/'output').is_file() for p in dirs))+',stderr='+str(sum((p/'stderr').is_file() for p in dirs)))
