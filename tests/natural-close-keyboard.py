import json
from playwright.sync_api import sync_playwright

with sync_playwright() as p:
    browser = p.chromium.launch(channel='msedge', headless=True)
    page = browser.new_page(viewport={'width': 1024, 'height': 720})
    page.goto('http://127.0.0.1:1459/')
    for _ in range(150):
        page.keyboard.press('Tab')
        if page.evaluate("document.activeElement.matches('.facts-header button') && document.activeElement.textContent.trim() === '打开解读'"):
            break
    else:
        raise AssertionError('opener unreachable')
    page.evaluate('''() => {
      window.openerButton = document.activeElement;
      window.collapseButton = [...document.querySelectorAll('.facts-header button')].find(e => e.textContent.trim() === '收起');
      if (!window.collapseButton) throw new Error('collapse button missing');
    }''')
    page.keyboard.press('Enter')
    page.wait_for_function("document.activeElement === document.querySelector('.insight-drawer [aria-label=关闭解读]')")
    page.evaluate('''() => {
      window.oldDrawer = document.querySelector('.insight-drawer');
      window.oldClose = document.activeElement;
    }''')
    assert page.locator('.insight-drawer [aria-label="关闭解读"]').is_visible()
    page.keyboard.press('Enter')
    page.wait_for_function("document.activeElement === window.openerButton && !window.oldDrawer.isConnected && !window.oldClose.isConnected && !document.querySelector('.insight-drawer')")
    for key, target in [('Tab', 'collapseButton'), ('Shift+Tab', 'openerButton')]:
        page.keyboard.press(key)
        state = page.evaluate('''target => {
          const e = document.activeElement, r = e.getBoundingClientRect();
          const x = (r.left + r.right) / 2, y = (r.top + r.bottom) / 2;
          const hit = document.elementFromPoint(x, y);
          return {tag: e.tagName, text: e.textContent.trim(), identity: e === window[target], connected: e.isConnected,
                  inViewport: r.left >= 0 && r.top >= 0 && r.right <= innerWidth && r.bottom <= innerHeight,
                  unobscured: !!hit && (hit === e || e.contains(hit))};
        }''', target)
        state['visible'] = page.locator(':focus').is_visible()
        state['enabled'] = page.locator(':focus').is_enabled()
        print(json.dumps({'key': key, **state}, ensure_ascii=True), flush=True)
        assert state['tag'] == 'BUTTON' and all(state[k] for k in ('identity', 'connected', 'inViewport', 'unobscured', 'visible', 'enabled'))
    page.keyboard.press('Enter')
    page.wait_for_function('''() => {
      const drawer = document.querySelector('.insight-drawer');
      const close = drawer?.querySelector('[aria-label="关闭解读"]');
      return drawer && drawer !== window.oldDrawer && close && close !== window.oldClose && document.activeElement === close;
    }''')
    assert page.locator('.insight-drawer [aria-label="关闭解读"]').is_visible()
    print(json.dumps({'reopened_new_drawer_and_close': True}), flush=True)
    browser.close()
