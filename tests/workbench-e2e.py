from playwright.sync_api import sync_playwright, expect

URL = 'http://127.0.0.1:1439/'

def check_size(browser, width, height):
    page = browser.new_page(viewport={'width': width, 'height': height})
    page.goto(URL)
    def keyboard_activate(target):
        for _ in range(100):
            if target.evaluate('(el) => el === document.activeElement'): break
            page.keyboard.press('Tab')
        expect(target).to_be_focused()
        page.keyboard.press('Enter')
    node = lambda id: page.locator(f'[data-node-id="{id}"]')
    drawer = page.locator('.insight-drawer')
    facts = page.locator('.facts')
    expect(node('login-api')).to_have_attribute('aria-pressed', 'true')
    opener = facts.get_by_role('button', name='打开解读')
    for _ in range(80):
        page.keyboard.press('Tab')
        if opener.evaluate('(el) => el === document.activeElement'): break
    expect(opener).to_be_focused()
    page.keyboard.press('Enter'); expect(drawer).to_be_visible()
    close = drawer.get_by_role('button', name='关闭解读')
    for _ in range(80):
        if close.evaluate('(el) => el === document.activeElement'): break
        page.keyboard.press('Tab')
    expect(close).to_be_focused(); page.keyboard.press('Enter'); expect(opener).to_be_focused()
    for tab, content in [('原文', '先校验邮箱和密码'), ('代码变更', 'findUserByEmail'), ('调用关系', 'src/auth/login.ts'), ('元信息', 'POST /api/login')]:
        facts.get_by_role('button', name=tab, exact=True).click()
        expect(facts.locator('.facts-content')).to_contain_text(content)
    facts.get_by_role('button', name='调用关系', exact=True).click()
    facts.get_by_role('button', name='src/auth/login.ts').click()
    expect(node('handler')).to_have_attribute('aria-pressed', 'true'); expect(node('handler')).to_be_focused()
    page.get_by_role('button', name='适应窗口').click()
    page.wait_for_timeout(200)
    node('login-api').dblclick()
    expect(drawer).to_contain_text('POST /api/login')
    geometry = drawer.evaluate('''el => { const a=el.getBoundingClientRect(), c=document.querySelector('.center-workspace').getBoundingClientRect(); return {left:a.left, right:a.right, centerRight:c.right} }''')
    if width > 1180: assert abs(geometry['left'] - geometry['centerRight']) < 2
    else: assert geometry['left'] < geometry['centerRight'] - 100
    if width > 1180:
        node('handler').click()
        expect(drawer).to_contain_text('POST /api/login')
        expect(drawer.get_by_role('button', name='原话', exact=True)).to_be_visible()
    if width <= 1180:
        candidate = page.locator('[data-node-id]:not([data-node-id=login-api])').evaluate_all('''els => { const d=document.querySelector('.insight-drawer').getBoundingClientRect(); return els.map(el => ({id:el.dataset.nodeId, r:el.getBoundingClientRect()})).find(x => x.r.left >= 186 && x.r.right < d.left && x.r.top >= 48 && x.r.bottom < innerHeight)?.id }''')
        assert candidate, 'no uncovered graph node available'
        node(candidate).click(); expect(node(candidate)).to_have_attribute('aria-pressed', 'true')
        expect(drawer).to_contain_text('POST /api/login'); expect(drawer.get_by_role('button', name='原话', exact=True)).to_be_visible()
        camera_before = page.locator('.react-flow__viewport').evaluate('(el) => getComputedStyle(el).transform')
        node(candidate).dblclick(); node(candidate).dblclick()
        drawer.get_by_role('button', name='关闭解读').click()
        expect(node(candidate)).to_be_focused()
        assert page.locator('.react-flow__viewport').evaluate('(el) => getComputedStyle(el).transform') == camera_before
        node('login-api').dblclick()
        for key in ['Tab', 'Shift+Tab']:
            for _ in range(30):
                page.keyboard.press(key)
                assert drawer.evaluate('(el) => el.contains(document.activeElement)')
    keyboard_activate(drawer.get_by_role('button', name='原话', exact=True))
    expect(node('login-api')).to_have_attribute('aria-pressed', 'true')
    expect(facts.get_by_role('button', name='原文', exact=True)).to_have_attribute('aria-pressed', 'true')
    item = facts.locator('[data-evidence-kind="quote"][data-evidence-id="auth:3"]')
    expect(item).to_be_focused(); expect(item).to_be_in_viewport()
    if width <= 1180: expect(drawer).to_have_count(0); node('login-api').dblclick()
    keyboard_activate(drawer.get_by_role('button', name='代码变更', exact=True))
    expect(node('login-api')).to_have_attribute('aria-pressed', 'true')
    expect(facts.get_by_role('button', name='代码变更', exact=True)).to_have_attribute('aria-pressed', 'true')
    item = facts.locator('[data-evidence-kind="diff"][data-evidence-id="login-diff"]')
    expect(item).to_be_focused(); expect(item).to_be_in_viewport()
    if width <= 1180: expect(drawer).to_have_count(0); node('login-api').dblclick()
    keyboard_activate(drawer.get_by_role('button', name='源码', exact=True))
    source = drawer.locator('pre').locator('..')
    expect(source).to_contain_text('随包材料 · src/auth/login.ts · 第 42 行'); expect(source.locator('pre')).to_contain_text('findUserByEmail'); expect(source.locator('pre')).to_contain_text('verifyPassword')
    page.set_viewport_size({'width': width, 'height': 400})
    scroll = drawer.evaluate('''el => { const body=el.querySelector('.insight-body'), head=el.querySelector('.insight-header'); const before=head.getBoundingClientRect().top; body.scrollTop=body.scrollHeight; return {before, after:head.getBoundingClientRect().top, top:body.scrollTop, max:body.scrollHeight-body.clientHeight} }''')
    assert abs(scroll['before']-scroll['after']) < 1
    assert scroll['max'] > 0 and scroll['top'] > 0, f'body did not scroll: {scroll}'
    page.set_viewport_size({'width': width, 'height': height})
    before_zoom = page.locator('.react-flow__viewport').evaluate('(el) => new DOMMatrix(getComputedStyle(el).transform).a')
    keyboard_activate(drawer.get_by_role('button', name='src/auth/password.ts'))
    expect(node('password')).to_have_attribute('aria-pressed', 'true'); expect(node('password')).to_be_focused(); expect(node('password')).to_be_in_viewport()
    page.wait_for_function('''() => { const a=document.querySelector('[data-node-id=password]').getBoundingClientRect(), b=document.querySelector('.react-flow').getBoundingClientRect(); return Math.abs((a.left+a.right-b.left-b.right)/2)<3 && Math.abs((a.top+a.bottom-b.top-b.bottom)/2)<3 }''')
    after_zoom = page.locator('.react-flow__viewport').evaluate('(el) => new DOMMatrix(getComputedStyle(el).transform).a')
    assert abs(before_zoom - after_zoom) < 0.001
    if width <= 1180: expect(drawer).to_have_count(0)
    node('login-api').dblclick()
    keyboard_activate(drawer.get_by_role('button', name='图中定位'))
    expect(node('login-api')).to_have_attribute('aria-pressed', 'true'); expect(node('login-api')).to_be_focused(); expect(node('login-api')).to_be_in_viewport()
    page.wait_for_function('''() => { const a=document.querySelector('[data-node-id=login-api]').getBoundingClientRect(), b=document.querySelector('.react-flow').getBoundingClientRect(); return Math.abs((a.left+a.right-b.left-b.right)/2)<3 && Math.abs((a.top+a.bottom-b.top-b.bottom)/2)<3 }''')
    if width <= 1180: expect(drawer).to_have_count(0)
    else:
        drawer.get_by_role('button', name='关闭解读').click()
        page.wait_for_timeout(100)
    opener = facts.get_by_role('button', name='打开解读')
    opener.focus(); page.keyboard.press('Enter'); expect(drawer).to_be_visible()
    close = drawer.get_by_role('button', name='关闭解读')
    if width > 1180:
        for _ in range(30):
            if close.evaluate('(el) => el === document.activeElement'): break
            page.keyboard.press('Tab')
        expect(close).to_be_focused()
    else: expect(close).to_be_focused()
    page.keyboard.press('Enter')
    expect(drawer).to_have_count(0); expect(opener).to_be_focused()
    if width > 1180:
        node('login').dblclick()
        expect(drawer).to_be_visible()
        calls = page.get_by_role('button', name='调用视图')
        calls.click()
        expect(node('login')).to_have_count(0)
        close = drawer.get_by_role('button', name='关闭解读')
        close.focus(); page.keyboard.press('Enter')
        expect(drawer).to_have_count(0); expect(calls).to_be_focused()
    page.get_by_role('button', name='订单导出功能').click(); expect(node('root')).to_have_attribute('aria-pressed', 'true')
    page.get_by_role('button', name='实现用户认证').click(); expect(node('login-api')).to_have_attribute('aria-pressed', 'true')
    page.get_by_role('button', name='从头演示').click(); expect(node('root')).to_have_attribute('aria-pressed', 'true'); expect(drawer).to_have_count(0)
    # Hold animation frames so graph focus is pending when the task changes.
    for destination in ('订单导出功能', '从头演示'):
        control = browser.new_page(viewport={'width': width, 'height': height})
        control.goto(URL)
        control.locator('[data-node-id=login-api]').dblclick()
        control.get_by_role('button', name=destination).click()
        control.wait_for_timeout(300)
        control_camera = control.locator('.react-flow__viewport').evaluate('(el) => getComputedStyle(el).transform')
        control.close()
        page.goto(URL)
        node('login-api').dblclick()
        page.evaluate('''() => { window.savedRAF = window.requestAnimationFrame; window.heldFrames = new Map(); window.nextFrame = 100000; window.savedCancel = window.cancelAnimationFrame; window.requestAnimationFrame = cb => { const id=window.nextFrame++; window.heldFrames.set(id, cb); return id }; window.cancelAnimationFrame = id => { window.heldFrames.delete(id); window.savedCancel(id) } }''')
        drawer.get_by_role('button', name='src/auth/password.ts').click()
        page.wait_for_timeout(100)
        assert page.evaluate('window.heldFrames.size') > 0, 'expected pending animation frame'
        page.get_by_role('button', name=destination).click()
        expect(node('root')).to_have_attribute('aria-pressed', 'true')
        page.evaluate('''() => { const held=[...window.heldFrames.values()]; window.requestAnimationFrame=window.savedRAF; window.cancelAnimationFrame=window.savedCancel; held.forEach(cb => window.savedRAF(cb)) }''')
        page.wait_for_timeout(300)
        expect(node('root')).to_have_attribute('aria-pressed', 'true')
        camera = page.locator('.react-flow__viewport').evaluate('(el) => getComputedStyle(el).transform')
        assert camera == control_camera, f'camera differs from control: {camera} vs {control_camera}'
        assert page.locator('[data-node-id=password][aria-pressed=true]').count() == 0
        assert not page.evaluate('document.activeElement?.dataset.nodeId === "password"')
    page.goto(URL)
    page.evaluate('''async () => {
      const R = (await import('/node_modules/.vite/deps/react.js')).default;
      const client = await import('/node_modules/.vite/deps/react-dom_client.js'); const createRoot = client.createRoot ?? client.default.createRoot;
      const {FactDrawer} = await import('/src/workspace/FactDrawer.tsx');
      const {scenarios} = await import('/src/demo/scenarios.ts');
      const {projectSession} = await import('/src/domain/project.ts');
      const host=document.createElement('div'); document.body.append(host); const root=createRoot(host);
      const sentinel=document.createElement('button'); document.body.append(sentinel);
      const noop=()=>{};
      const render=async (scenario,count,focus) => { root.render(R.createElement(FactDrawer,{key:scenario.id,session:projectSession(scenario.events,count),selectedId:scenario.id==='auth'?'login-api':'handler',expanded:true,tab:'diff',focus,onToggle:noop,onTab:noop,onLocate:noop,onOpenInsight:noop})); await new Promise(r=>setTimeout(r,80)); };
      await render(scenarios[0],3,{requestId:1,kind:'diff',evidenceId:'login-diff'});
      if (host.querySelector('[data-evidence-id=login-diff]')) throw Error('expected pending evidence');
      await render(scenarios[1],8,null); sentinel.focus();
      await render(scenarios[0],4,null);
      if (!host.querySelector('[data-evidence-id=login-diff]')) throw Error('evidence missing after switch');
      if (document.activeElement!==sentinel) throw Error('old FactFocus stole focus after switch');
      root.unmount(); host.remove(); sentinel.remove();
    }''')
    page.close()
    print(f'PASS {width}x{height}', flush=True)

with sync_playwright() as p:
    browser = p.chromium.launch(channel='msedge', headless=True)
    for size in [(1440, 900), (1024, 720)]: check_size(browser, *size)
    browser.close()
