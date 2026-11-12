import { useEffect, useReducer } from 'react'
import { scenarios } from './demo/scenarios'
import { createWorkspace, workspaceReducer } from './workspace/state'
import { Workbench } from './workspace/Workbench'
import './workspace/workbench.css'
export default function App() {
  const [state, dispatch] = useReducer(workspaceReducer, scenarios[0], createWorkspace)
  const { playing, runId, appliedCount } = state
  useEffect(() => {
    if (!playing) return
    const timer = window.setTimeout(() => dispatch({ type: 'tick', runId, expectedCount: appliedCount }), 1000)
    return () => window.clearTimeout(timer)
  }, [playing, runId, appliedCount])
  return <Workbench state={state} dispatch={dispatch} />
}
