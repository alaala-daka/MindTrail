from playwright.sync_api import sync_playwright

with sync_playwright() as p:
    browser = p.chromium.launch(channel='msedge', headless=True)
    for size in ((1440, 900), (1024, 720)):
        page = browser.new_page(viewport={'width': size[0], 'height': size[1]})
        page.goto('http://127.0.0.1:4188/tests/productization-harness.html')
        button = lambda name: page.get_by_role('button', name=name, exact=True)
        button('准备解读').click()
        assert page.get_by_text('待发送的合成事实').count() == 1
        assert page.get_by_text('https://example.invalid', exact=False).count() == 1
        button('读取计数').click(); assert page.locator('output').inner_text() == '0:0'
        button('发送并生成').click(); button('取消生成').click(); button('完成模拟').click()
        assert page.get_by_text('合成结果').count() == 0
        button('读取计数').click(); assert page.locator('output').inner_text() == '1:1'
        button('准备解读').click(); button('发送并生成').click(); button('完成模拟').click()
        page.get_by_text('合成结果').wait_for()
        assert page.evaluate('window.__injected === undefined')
        button('原话').click(); assert page.locator('output').inner_text() == 'reveal:auth:3'
        button('准备解读').click(); button('发送并生成').click(); button('切换前缀').click(); button('完成模拟').click()
        assert page.get_by_text('合成结果').count() == 0
        button('未配置').click(); page.get_by_text('尚未配置 AI 服务。').wait_for()
        assert page.get_by_text('事实清单').count() == 1
        assert button('准备解读').count() == 0
        if size[0] == 1024:
            button('关闭解读').focus(); page.keyboard.press('Tab')
            assert page.evaluate("document.querySelector('.insight-drawer').contains(document.activeElement)")
            page.keyboard.press('Shift+Tab')
            assert page.evaluate("document.querySelector('.insight-drawer').contains(document.activeElement)")
            page.keyboard.press('Escape'); assert page.get_by_label('节点解读').count() == 0
        page.close()
    for change in ('切换节点', '切换库', '卸载抽屉'):
        page = browser.new_page()
        page.goto('http://127.0.0.1:4188/tests/productization-harness.html')
        button = lambda name: page.get_by_role('button', name=name, exact=True)
        button('准备解读').click(); button('发送并生成').click(); button(change).click(); button('完成模拟').click()
        assert page.get_by_text('合成结果').count() == 0
        button('读取计数').click(); assert page.locator('output').inner_text() == '1:1'
        page.close()
    page = browser.new_page()
    page.goto('http://127.0.0.1:4188/tests/productization-harness.html')
    button = lambda name: page.get_by_role('button', name=name, exact=True)
    for _ in range(12):
        button('准备解读').click(); button('取消').click()
    button('读取票据').click(); assert page.locator('output').inner_text() == 'tickets:0'
    page.close()
    page = browser.new_page()
    page.goto('http://127.0.0.1:4188/tests/productization-harness.html')
    button = lambda name: page.get_by_role('button', name=name, exact=True)
    button('故障 wrong-context').click(); button('准备解读').click()
    page.get_by_text('预览已过期，请重新准备。').wait_for()
    button('读取票据').click(); assert page.locator('output').inner_text() == 'tickets:0'
    page.close()
    page = browser.new_page()
    page.goto('http://127.0.0.1:4188/tests/productization-harness.html')
    button = lambda name: page.get_by_role('button', name=name, exact=True)
    button('故障 defer-prepare').click(); button('准备解读').click(); button('卸载抽屉').click(); button('完成准备').click()
    button('读取票据').click(); assert page.locator('output').inner_text() == 'tickets:0'
    page.close()
    for code, message in [('unauthorized','服务未授权，请检查配置。'), ('rate_limited','服务请求受限，请稍后重试。'), ('timeout','请求超时，请重试。'), ('transport','无法连接 AI 服务，请重试。'), ('expired','预览已过期，请重新准备。')]:
        page = browser.new_page()
        page.goto('http://127.0.0.1:4188/tests/productization-harness.html')
        button = lambda name: page.get_by_role('button', name=name, exact=True)
        button('故障 ' + code).click(); button('准备解读').click(); button('发送并生成').click()
        page.get_by_text(message).wait_for(); assert button('准备解读').count() == 1
        page.close()
    page = browser.new_page(viewport={'width': 1024, 'height': 720})
    page.goto('http://127.0.0.1:4188/')
    opener = page.get_by_role('button', name='打开解读', exact=True)
    opener.focus(); page.keyboard.press('Enter')
    page.get_by_role('button', name='关闭解读', exact=True).wait_for()
    page.keyboard.press('Escape')
    page.wait_for_function("document.activeElement?.textContent?.trim() === '打开解读'")
    page.close()
    page=browser.new_page()
    page.goto('http://127.0.0.1:4188/tests/connection-harness.html')
    page.get_by_text('网关：可用').wait_for()
    page.get_by_role('button',name='复制配置',exact=True).click()
    page.get_by_text('配置已复制。').wait_for()
    page.get_by_role('button',name='检查剪贴板',exact=True).click()
    copied=page.locator('output').inner_text()
    assert '[mcp_servers.mindtrail]' in copied and 'mindtrail-producer.exe' in copied and 'token' not in copied.lower()
    page.get_by_role('button',name='模拟复制失败',exact=True).click()
    page.get_by_role('button',name='复制配置',exact=True).click()
    page.get_by_text('复制失败，请手动选择配置文本。').wait_for()
    page.get_by_role('button',name='本机自检',exact=True).click()
    page.get_by_text('仅检查本机状态，尚未证明 Agent 或 AI 已接入。',exact=False).wait_for()
    page.close()
    page=browser.new_page()
    page.goto('http://127.0.0.1:4188/tests/connection-harness.html?missing=1')
    page.get_by_text('连接程序尚未随此版本安装或路径不可用。').wait_for()
    assert page.get_by_role('button',name='复制配置',exact=True).count()==0
    page.get_by_role('button',name='本机自检',exact=True).click()
    page.get_by_text('本机检查未通过。',exact=False).wait_for()
    page.close()
    page=browser.new_page(viewport={'width':1440,'height':900})
    page.goto('http://127.0.0.1:4188/tests/welcome-harness.html')
    page.get_by_role('button',name='真实合成任务',exact=True).click()
    page.locator('.topbar-title .badge').get_by_text('真实任务').wait_for()
    page.get_by_role('button',name='连接与任务',exact=True).click()
    page.get_by_role('button',name='演示',exact=True).click()
    page.locator('.topbar-title .badge').get_by_text('内置样例').wait_for(timeout=3000)
    assert '真实合成任务' not in page.locator('.topbar-title').inner_text()
    page.close()
    browser.close()
print('productization browser checks passed')
