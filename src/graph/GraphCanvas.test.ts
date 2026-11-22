import { expect, it } from 'vitest'
import { Position } from '@xyflow/react'
import { cardHandles } from './GraphCanvas'
import { NODE_HEIGHT, NODE_WIDTH } from './layout'

it('defines stable connection bounds at both ends of every fixed-size card', () => {
  expect(cardHandles()).toEqual([
    { type: 'target', position: Position.Left, x: 0, y: NODE_HEIGHT / 2, width: 1, height: 1 },
    { type: 'source', position: Position.Right, x: NODE_WIDTH, y: NODE_HEIGHT / 2, width: 1, height: 1 },
  ])
})
