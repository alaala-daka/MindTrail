from playwright.sync_api import sync_playwright

with sync_playwright() as p:
    browser = p.chromium.launch(channel='msedge', headless=True)
    page = browser.new_page()
    page.goto('http://127.0.0.1:4188/tests/insight-harness.html')
    button = lambda name: page.get_by_role('button', name=name, exact=True)
    assert page.get_by_text('代码变更（暂不可用）').count() == 1
    assert page.get_by_text('源码（暂不可用）').count() == 1
    for prefix, handler, password in [(5, 0, 0), (6, 1, 0), (7, 1, 1)]:
        button(f'prefix {prefix}').click()
        assert button('src/auth/login.ts').count() == handler
        assert button('src/auth/password.ts').count() == password
    button('原话').focus()
    page.keyboard.press('Enter')
    assert page.locator('output').inner_text() == 'reveal:quote:auth:3'
    button('代码变更').click()
    assert page.locator('output').inner_text() == 'reveal:diff:login-diff'
    button('源码').click()
    assert page.get_by_text('随包材料 · src/auth/login.ts · 第 42 行').count() == 1
    assert 'await findUserByEmail(email)' in page.locator('pre').inner_text()
    button('src/auth/password.ts').focus()
    page.keyboard.press('Enter')
    assert page.locator('output').inner_text() == 'locate:password'
    button('handler node').click()
    assert page.get_by_text('事实清单').count() == 1
    assert page.get_by_text('上下游关系').count() == 1
    button('invalid node').click()
    assert page.get_by_text('节点尚不可用').count() == 1
    button('关闭解读').click()
    assert page.locator('output').inner_text() == 'close'
    browser.close()
    print('InsightDrawer Edge interaction checks passed')
