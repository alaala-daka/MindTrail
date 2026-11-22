from playwright.sync_api import sync_playwright
with sync_playwright() as p:
 b=p.chromium.launch(channel='msedge',headless=True)
 for case in ['normal','tab']:
  page=b.new_page(viewport={'width':900,'height':720}); page.goto('http://127.0.0.1:1457/')
  for i in range(100):
   a=page.evaluate("document.activeElement.outerHTML")
   if page.evaluate("document.activeElement?.tagName === 'BUTTON' && document.activeElement.textContent.includes('打开解读')"): break
   page.keyboard.press('Tab')
  else: raise Exception('opener unreachable')
  print(case,'opener',a); page.evaluate('window.testOpener = document.activeElement'); page.keyboard.press('Enter')
  page.evaluate("""() => { const original=window.requestAnimationFrame; window.held=[]; window.requestAnimationFrame=cb=>String(cb).includes('closeFocusSeq')?(held.push(cb),999999):original(cb); window.releaseHeld=()=>{window.requestAnimationFrame=original; held.splice(0).forEach(cb=>cb(performance.now()))} }""")
  for i in range(100):
   a=page.evaluate('document.activeElement.outerHTML')
   if page.evaluate("document.activeElement?.tagName === 'BUTTON' && document.activeElement.getAttribute('aria-label') === '关闭解读'"): break
   page.keyboard.press('Tab')
  else: raise Exception('close unreachable')
  page.evaluate('window.testCloseButton = document.activeElement')
  page.keyboard.press('Enter'); assert page.evaluate('held.length')==1; assert page.evaluate('!window.testCloseButton.isConnected')
  if case=='tab':
   page.keyboard.press('Tab'); print('first tab:', page.evaluate('document.activeElement.tagName')); page.keyboard.press('Tab'); print('second tab:', page.evaluate('document.activeElement.outerHTML')); assert page.evaluate("document.activeElement === document.querySelector('.segmented button[aria-pressed=true]') && document.activeElement.textContent.trim() === '层级视图' && document.activeElement !== window.testOpener"); page.evaluate('window.expected=document.activeElement')

  page.evaluate('releaseHeld()')
  print('after release',page.evaluate('document.activeElement.outerHTML'))
  if case=='tab': assert page.evaluate('document.activeElement===window.expected')
  else: assert page.evaluate('document.activeElement === window.testOpener')
  page.close()
 b.close()
