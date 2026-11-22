import { useState } from 'react'
import { createRoot } from 'react-dom/client'
import { scenarios } from '../src/demo/scenarios'
import { projectSession } from '../src/domain/project'
import { InsightDrawer } from '../src/workspace/InsightDrawer'
function Harness() {
  const [count, setCount] = useState(3)
  const [node, setNode] = useState('login-api')
  const [result, setResult] = useState('')
  return <>{[3, 5, 6, 7].map(n => <button key={n} onClick={() => setCount(n)}>prefix {n}</button>)}<button onClick={() => setNode('login-api')}>login</button><button onClick={() => setNode('handler')}>handler node</button><button onClick={() => setNode('missing')}>invalid node</button><output>{result}</output><InsightDrawer source="demo" openSeq={1} session={projectSession(scenarios[0].events, count)} nodeId={node} onClose={() => setResult('close')} onLocateNode={id => setResult(`locate:${id}`)} onRevealEvidence={ref => setResult(`reveal:${ref.kind}:${ref.id}`)} /></>
}
createRoot(document.getElementById('root')!).render(<Harness />)
