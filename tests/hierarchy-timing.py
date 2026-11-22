import json
from playwright.sync_api import sync_playwright

with sync_playwright() as p:
    print('Launching headless Edge', flush=True)
    browser = p.chromium.launch(executable_path=r'C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe', headless=True)
    page = browser.new_page(viewport={'width':1440,'height':900})
    page.add_init_script('''(() => {
      const original = window.setTimeout;
      const records = window.records = []; let pending = null;
      const progress = () => document.querySelector('.progress')?.textContent.trim();
      const node = id => document.querySelector('.react-flow [data-node-id="' + id + '"]');
      const state = (id, text) => !!node(id)?.textContent.includes(text);
      const checks = {2:()=>state('login','计划'),3:()=>!!node('login-api'),4:()=>!!node('handler')&&state('login','执行中'),5:()=>!!node('password'),8:()=>state('login','完成'),9:()=>state('session','计划'),10:()=>!!node('session-file')&&state('session','执行中'),11:()=>state('middleware','计划'),12:()=>!!node('refresh-api'),13:()=>state('session','完成'),14:()=>!!node('middleware-file')&&state('middleware','执行中'),15:()=>state('middleware','完成')};
      window.setTimeout = function(fn, delay, ...args) {
        if (delay !== 1000 || typeof fn !== 'function' || !fn.toString().includes('tick')) return original.call(this, fn, delay, ...args);
        const registered = performance.now();
        return original.call(this, function() {
          const start = performance.now();
          if (pending) { pending.observer.disconnect(); pending.closed = true; pending = null; }
          const before = progress(); const match = /^(\\d+) \\/ 16$/.exec(before || '');
          const seq = match ? Number(match[1])+1 : null;
          const check = checks[seq];
          const rec = {seq, before, registered, start, baseline: check ? !!check() : null, after:null, ms:null, closed:false}; records.push(rec);
          const graph = document.querySelector('.react-flow');
          const inspect = () => {
            if (!check || rec.baseline || !check()) return;
            observer.disconnect(); pending = null;
            requestAnimationFrame(() => { rec.ms = performance.now()-start; rec.after = progress(); });
          };
          const observer = new MutationObserver(inspect);
          if (check && graph) { observer.observe(graph, {subtree:true,childList:true,characterData:true,attributes:true}); pending = {observer, get closed(){return rec.closed}, set closed(v){rec.closed=v}}; }
          const result = fn(...args);
          if (seq === 16) requestAnimationFrame(() => { rec.after = progress(); });
          return result;
        }, delay);
      };
    })();''')
    print('Opening http://localhost:1441', flush=True)
    page.goto('http://localhost:1441', wait_until='networkidle', timeout=15000)
    page.get_by_role('button', name='从头演示').click()
    page.wait_for_timeout(17000)
    records = page.evaluate('window.records')
    print(json.dumps(records, ensure_ascii=False), flush=True)
    assert [r['seq'] for r in records] == list(range(2,17)), 'callbacks missing, duplicated, or progress misaligned'
    measured = set(range(2,16)) - {6,7}
    for r in records:
        assert r['start'] - r['registered'] >= 990, f"timer fired early: {r}"
        if r['seq'] in measured:
            assert r['baseline'] is False and r['ms'] is not None and r['ms'] < 1000 and not r['closed'], f"invalid measurement: {r}"
            assert r['after'] == f"{r['seq']} / 16", f"progress mismatch: {r}"
        if r['seq'] == 16: assert r['after'] == '演示完成', f"completion missing: {r}"
    print('PASS', flush=True)
    browser.close()
