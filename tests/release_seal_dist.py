"""Seal the already verified frontend output in an owned run."""
from pathlib import Path
import sys

ROOT=Path(__file__).resolve().parent.parent
sys.path.insert(0,str(ROOT/'tests'))
from release_desktop_preflight import main as preflight
from release_desktop_candidate import fingerprint_tree

if __name__=='__main__':
    try:
        run_id=sys.argv[1]
        if preflight(run_id,True)!=0: raise ValueError('preflight_failed')
        run=ROOT/'.tools/productization-release'/('run-'+run_id)
        digest=fingerprint_tree(run/'source/dist')
        with (run/'frontend-dist.sha256').open('x',encoding='ascii') as f: f.write(digest+'\n')
        print('frontend_dist_sealed=true')
    except Exception:
        print('frontend_dist_sealed=false'); sys.exit(1)
