import json
from playwright.sync_api import sync_playwright

with sync_playwright() as p:
    print('Launching headless Edge', flush=True)
    browser = p.chromium.launch(executable_path=r'C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe', headless=True)
    page = browser.new_page(viewport={'width':1440,'height':900})
    page.add_init_script(r'''(() => {
      const original = window.setTimeout;
      const records = window.records = []; let pending = null;
      const progress = () => document.querySelector('.progress')?.textContent.trim();
      const node = id => document.querySelector('.react-flow [data-node-id="' + id + '"]');
      const state = (id, text) => !!node(id)?.textContent.includes(text);
      const mode = () => document.querySelector('.segmented button[aria-pressed="true"]')?.textContent.trim() === '调用视图';
      // React Flow ArrowClosed is a closed polyline with its tip at the origin.
      const closedArrow = marker => {
        const shape = marker?.querySelector('polyline.arrowclosed');
        const nums = (shape?.getAttribute('points') || '').trim().split(/[\s,]+/).map(Number);
        if (nums.length !== 8 || nums.some(n => !Number.isFinite(n))) return false;
        const [x1,y1,x2,y2,x3,y3,x4,y4] = nums;
        const area = Math.abs((x1*y2-x2*y1)+(x2*y3-x3*y2)+(x3*y1-x1*y3))/2;
        return x1 === x4 && y1 === y4 && area > 0 && x2 === 0 && y2 === 0 && x1 < x2 && x3 < x2 && y1 * y3 < 0;
      };
      window.testClosedArrow = points => { const m = document.createElementNS('http://www.w3.org/2000/svg', 'marker'); const p = document.createElementNS('http://www.w3.org/2000/svg', 'polyline'); p.setAttribute('class', 'arrowclosed'); p.setAttribute('points', points); m.appendChild(p); return closedArrow(m); };
      const edge = (id, label, from, to) => {
        const e = [...document.querySelectorAll('.react-flow__edge')].find(x => x.getAttribute('data-id') === id);
        const path = e?.querySelector('path.react-flow__edge-path');
        const markerId = /^url\(['\"]?#([^'\")]+)['\"]?\)$/.exec(path?.getAttribute('marker-end') || '')?.[1];
        const marker = markerId && document.getElementById(markerId);
        if (!e?.textContent.includes(label) || !path || !marker || marker.tagName.toLowerCase() !== 'marker' || !['auto', 'auto-start-reverse'].includes(marker.getAttribute('orient')) || !closedArrow(marker) || e.getAttribute('aria-label') !== `Edge from ${from} to ${to}`) return false;
        const matrix = path.getScreenCTM(), length = path.getTotalLength();
        if (!matrix || !length) return false;
        const point = at => new DOMPoint(path.getPointAtLength(at).x, path.getPointAtLength(at).y).matrixTransform(matrix);
        const center = id => { const r = node(id)?.getBoundingClientRect(); return r && {x:r.x+r.width/2,y:r.y+r.height/2}; };
        const start = point(0), end = point(length), source = center(from), target = center(to);
        if (!source || !target) return false;
        const dist = (a,b) => Math.hypot(a.x-b.x,a.y-b.y);
        return dist(start,source) < dist(start,target) && dist(end,target) < dist(end,source);
      };
      const checks = {3:()=>!!node('login-api'),4:()=>!!node('handler'),5:()=>!!node('password'),6:()=>edge('login-api:calls:handler','调用','login-api','handler'),7:()=>edge('handler:imports:password','引用','handler','password'),10:()=>!!node('session-file'),12:()=>!!node('refresh-api'),14:()=>!!node('middleware-file')};
      window.setTimeout = function(fn, delay, ...args) {
        if (delay !== 1000 || typeof fn !== 'function' || !fn.toString().includes('tick')) return original.call(this, fn, delay, ...args);
        const registered = performance.now();
        return original.call(this, function() {
          const start = performance.now();
          if (pending) { pending.observer.disconnect(); pending.closed = true; pending = null; }
          const before = progress(); const match = /^(\d+) \/ 16$/.exec(before || '');
          const seq = match ? Number(match[1])+1 : null;
          const check = checks[seq];
          const rec = {seq, before, registered, start, baseline: check ? !!check() : null, after:null, ms:null, closed:false, modeBefore:mode(), modeAfter:null, stable:null}; records.push(rec);
          const graph = document.querySelector('.react-flow');
          const inspect = () => {
            if (!check || rec.baseline || !check()) return;
            observer.disconnect(); pending = null;
            requestAnimationFrame(() => { rec.ms = performance.now()-start; rec.after = progress(); rec.stable = !!check(); rec.modeAfter = mode(); });
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
    assert page.evaluate("testClosedArrow('-5,-4 0,0 -5,4 -5,-4')")
    for points in ['', '-5,-4 0,0 -5,4', '5,-4 0,0 5,4 5,-4']:
        assert not page.evaluate('(points) => testClosedArrow(points)', points)
    page.get_by_role('button', name='从头演示').click()
    page.get_by_role('button', name='调用视图').click()
    assert page.get_by_role('button', name='调用视图').get_attribute('aria-pressed') == 'true'
    assert page.locator('.progress').inner_text() == '1 / 16'
    page.wait_for_timeout(17000)
    records = page.evaluate('window.records')
    print(json.dumps(records, ensure_ascii=False), flush=True)
    assert [r['seq'] for r in records] == list(range(2,17)), 'callbacks missing, duplicated, or progress misaligned'
    measured = {3,4,5,6,7,10,12,14}
    for r in records:
        assert r['modeBefore'], f"wrong view: {r}"
        assert r['start'] - r['registered'] >= 990, f"timer fired early: {r}"
        if r['seq'] in measured:
            assert r['stable'] and r['modeAfter'] and r['baseline'] is False and r['ms'] is not None and r['ms'] < 1000 and not r['closed'], f"invalid measurement: {r}"
            assert r['after'] == f"{r['seq']} / 16", f"progress mismatch: {r}"
        if r['seq'] == 16: assert r['after'] == '演示完成', f"completion missing: {r}"
    assert page.get_by_role('button', name='调用视图').get_attribute('aria-pressed') == 'true'
    print('PASS', flush=True)
    browser.close()
