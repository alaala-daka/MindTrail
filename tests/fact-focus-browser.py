"""Run against the Vite server at 127.0.0.1:1420 with Python Playwright and Edge."""
from playwright.sync_api import sync_playwright

with sync_playwright() as p:
    browser = p.chromium.launch(channel='msedge', headless=True)
    page = browser.new_page(viewport={'width': 1440, 'height': 900})
    page.goto('http://127.0.0.1:1420/', wait_until='domcontentloaded')
    result = page.evaluate('''async () => {
      const reactModule = await import('/node_modules/.vite/deps/react.js'); const React = reactModule.default ?? reactModule;
      const client = await import('/node_modules/.vite/deps/react-dom_client.js'); const createRoot = client.createRoot ?? client.default.createRoot;
      const { FactDrawer } = await import('/src/workspace/FactDrawer.tsx');
      const { scenarios } = await import('/src/demo/scenarios.ts');
      const { projectSession } = await import('/src/domain/project.ts');
      const host = document.createElement('div'); document.body.append(host);
      const root = createRoot(host);
      let session = projectSession(scenarios[0].events, 3), tab = 'diff', expanded = true;
      let focus = {requestId: 1, kind: 'diff', evidenceId: 'login-diff'};
      const noop = () => {};
      const render = async () => { root.render(React.createElement(FactDrawer, {key: session.id, session, selectedId:'login-api', expanded, tab, focus, onToggle:noop, onTab:noop, onLocate:noop, onOpenInsight:noop})); await new Promise(resolve => setTimeout(resolve, 60)); };
      const assert = (condition, message) => { if (!condition) throw Error(message); };
      await render(); assert(!host.querySelector('[data-evidence-id="login-diff"]'), 'evidence should be pending');
      session = projectSession(scenarios[0].events, 4); await render();
      let item = host.querySelector('[data-evidence-id="login-diff"]'); assert(document.activeElement === item, 'pending request should focus when evidence appears');
      tab = 'quotes'; await render(); host.querySelector('button').focus(); tab = 'diff'; await render();
      assert(document.activeElement !== host.querySelector('[data-evidence-id="login-diff"]'), 'tab change stole focus');
      expanded = false; await render(); expanded = true; await render();
      assert(document.activeElement !== host.querySelector('[data-evidence-id="login-diff"]'), 'expand stole focus');
      focus = {...focus, requestId: 2}; await render(); assert(document.activeElement === host.querySelector('[data-evidence-id="login-diff"]'), 'new request did not focus');
      session = projectSession(scenarios[1].events, 8); focus = null; await render();
      session = projectSession(scenarios[0].events, 4); focus = {requestId: 2, kind:'diff', evidenceId:'login-diff'}; await render();
      assert(document.activeElement === host.querySelector('[data-evidence-id="login-diff"]'), 'new session lifecycle did not reset');
      root.unmount(); host.remove(); return 'PASS: pending, tabs, collapse, new request, session lifecycle';
    }''')
    print(result)
    browser.close()
