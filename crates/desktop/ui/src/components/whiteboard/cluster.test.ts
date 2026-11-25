import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { HIDE_MS, clusterTimer } from './cluster-timer'

describe('cluster timer', () => {
  beforeEach(() => vi.useFakeTimers())
  afterEach(() => vi.useRealTimers())

  const rig = (keep = false) => {
    const changes: boolean[] = []
    const t = clusterTimer({ onChange: (o) => changes.push(o), keep })
    return { t, changes }
  }

  it('a tap raises it; it hides after 4 s without finger input', () => {
    const { t, changes } = rig()
    t.tap()
    expect(t.isOpen()).toBe(true)
    vi.advanceTimersByTime(HIDE_MS - 1)
    expect(t.isOpen()).toBe(true)
    vi.advanceTimersByTime(1)
    expect(t.isOpen()).toBe(false)
    expect(changes).toEqual([true, false])
  })

  it('finger input on the cluster restarts the 4 s', () => {
    const { t } = rig()
    t.grabber()
    vi.advanceTimersByTime(3_000)
    t.activity()
    vi.advanceTimersByTime(3_000)
    expect(t.isOpen()).toBe(true)
    vi.advanceTimersByTime(1_000)
    expect(t.isOpen()).toBe(false)
  })

  it('pen activity hides it at once', () => {
    const { t } = rig()
    t.tap()
    t.pen()
    expect(t.isOpen()).toBe(false)
  })

  it('a second tap hides it', () => {
    const { t, changes } = rig()
    t.tap()
    t.tap()
    expect(t.isOpen()).toBe(false)
    expect(changes).toEqual([true, false])
  })

  it('keepControls stops the timer; turning it off starts it again', () => {
    const { t } = rig(true)
    t.tap()
    vi.advanceTimersByTime(60_000)
    expect(t.isOpen()).toBe(true)
    t.keep(false)
    vi.advanceTimersByTime(HIDE_MS)
    expect(t.isOpen()).toBe(false)
  })

  it('an open popover holds it up past 4 s; closing restarts the 4 s; a pen still hides it', () => {
    const { t } = rig()
    t.tap()
    t.hold(true)
    vi.advanceTimersByTime(60_000)
    expect(t.isOpen()).toBe(true)
    t.hold(false)
    vi.advanceTimersByTime(HIDE_MS - 1)
    expect(t.isOpen()).toBe(true)
    vi.advanceTimersByTime(1)
    expect(t.isOpen()).toBe(false)
    t.tap()
    t.hold(true)
    t.pen()
    expect(t.isOpen()).toBe(false)
  })

  it('activity while hidden does not raise it', () => {
    const { t } = rig()
    t.activity()
    expect(t.isOpen()).toBe(false)
  })
})
