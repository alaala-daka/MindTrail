from playwright.sync_api import sync_playwright, expect

URL = 'http://localhost:1441/'


def run_case(browser, width, case):
    page = browser.new_page(viewport={'width': width, 'height': 900})
    try:
        page.goto(URL)
        opener = page.locator('.facts').get_by_role('button', name='打开解读')
        if case in ('unmounted', 'no_selection'):
            opener = page.locator('[data-node-id="login"]')
            opener.dblclick()
            page.evaluate("window.originalOpener = document.querySelector('[data-node-id=login]')")
            page.get_by_role('button', name='调用视图').evaluate('(el) => el.click()')
            expect(opener).to_have_count(0)
            if case == 'unmounted':
                page.locator('[data-node-id="login-api"]').evaluate('(el) => el.click()')
            else:
                assert page.locator('.canvas button[data-node-id][aria-pressed=true]').count() == 0
        else:
            opener.click()
        page.evaluate('''() => { const original = window.requestAnimationFrame; window.held = []; window.requestAnimationFrame = cb => { if (String(cb).includes('closeFocusSeq')) { window.held.push(cb); return 999999; } return original(cb); }; window.releaseHeld = () => { window.requestAnimationFrame = original; window.held.splice(0).forEach(cb => cb(performance.now())); }; }''')
        if case == 'keyboard':
            page.get_by_role('button', name='调用视图').focus()
            page.get_by_role('button', name='关闭解读').evaluate('(el) => el.click()')
        else:
            page.get_by_role('button', name='关闭解读').click()
        assert page.evaluate('held.length') == 1, 'close callback was not captured'
        if case in ('fallback', 'unmounted', 'no_selection'):
            if case == 'fallback':
                opener.evaluate("el => el.style.display = 'none'")
            immediate = page.evaluate('''() => { releaseHeld(); return document.activeElement === document.querySelector('.canvas button[data-node-id][aria-pressed=true]'); }''') if case != 'no_selection' else page.evaluate('''() => { releaseHeld(); return document.activeElement === document.querySelector('.segmented button[aria-pressed=true]'); }''')
            assert immediate, 'fallback did not focus synchronously'
            target = page.get_by_role('button', name='调用视图')
            target.click()
            page.wait_for_timeout(300)
            expect(target).to_be_focused()
            print(f'PASS width={width} case={case}')
            return
        elif case == 'focusin':
            target = page.get_by_role('button', name='调用视图')
            target.focus()
            expect(target).to_be_focused()
            page.evaluate('releaseHeld()')
            expect(target).to_be_focused()
            print(f'PASS width={width} case={case}')
            return
        elif case == 'blur':
            page.evaluate("window.dispatchEvent(new Event('blur'))")
            page.evaluate('window.expectedFocus = document.activeElement')
            page.evaluate('releaseHeld()')
            assert page.evaluate('document.activeElement === window.expectedFocus'), 'blur did not cancel restoration'
            print(f'PASS width={width} case={case}')
            return
        elif case == 'normal':
            target = opener
        elif case == 'keyboard':
            page.keyboard.press('Tab')
            target = page.locator(':focus')
            assert target.count() == 1, 'Tab did not move focus'
            page.evaluate('window.expectedFocus = document.activeElement')
        elif case == 'node':
            target = page.locator('[data-node-id="handler"]')
            target.click()
        else:
            target = (page.locator('.task-list') if case == 'task' else page).get_by_role('button', name={'view': '调用视图', 'task': '订单导出功能', 'restart': '从头演示'}[case])
            target.click()
        if case != 'normal':
            expect(target).to_be_focused()
        page.evaluate('releaseHeld()')
        page.wait_for_timeout(100)
        if case == 'keyboard':
            assert page.evaluate('document.activeElement === window.expectedFocus'), 'keyboard focus was stolen'
        else:
            expect(target).to_be_focused()
        print(f'PASS width={width} case={case}')
    finally:
        page.close()


with sync_playwright() as p:
    browser = p.chromium.launch(channel='msedge', headless=True)
    try:
        for width in (1600, 900):
            cases = ('normal', 'node', 'view', 'keyboard', 'task', 'restart', 'fallback', 'blur', 'focusin', 'unmounted', 'no_selection') if width == 1600 else ('normal', 'node', 'view', 'task', 'restart', 'fallback', 'blur', 'focusin', 'unmounted', 'no_selection')
            for case in cases:
                run_case(browser, width, case)
    finally:
        browser.close()
