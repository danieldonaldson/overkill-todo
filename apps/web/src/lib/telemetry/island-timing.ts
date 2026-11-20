// Panel timing (OT-006 Decision 6b): when an island became interactive and
// when it was ready, from the start of the navigation that showed it.
//
// A panel that loads data creates the handle in its script and passes it to
// its root `TelemetryBoundary`, then calls `markReady()` once it first shows
// its content, its empty state or its error state:
//
//   const timing = createIslandTiming()
//   …load… then timing.markReady()
//   <TelemetryBoundary name="AdminPanel" {timing}>
//
// (A handle, not Svelte context: the boundary is the panel's own root, so the
// panel's script sits outside any context the boundary could provide.) An
// island with nothing to load passes no handle; its boundary marks it ready
// when it mounts.
//
// Both marks are dropped when this page load isn't sampled, or when a newer
// navigation started since the island was created (the user moved on), so a
// superseded navigation's panels aren't recorded. `ready` is sent once.

import { reportIslandTiming } from './index'
import { currentNavigation, timingSampled } from './navigation'

export type IslandStage = 'interactive' | 'ready'

/**
 * A panel's timing handle. The panel calls `markReady()`; its boundary calls
 * `attach()` and `markInteractive()`.
 */
export class IslandTimer {
  /** The navigation that showed this island, fixed when it was created. */
  private readonly navigation = currentNavigation()
  private name: string | null = null
  private readyAt: number | null = null
  private readySent = false
  private interactiveSent = false

  constructor(private readonly now: () => number = () => performance.now()) {}

  /** Give the island its name (the boundary's). Sends a ready mark made before. */
  attach(name: string): void {
    this.name ??= name
    this.sendReady()
  }

  /** The island mounted. Sent once. */
  markInteractive(): void {
    if (this.interactiveSent) return
    this.interactiveSent = true
    this.send('interactive', this.now())
  }

  /** The panel shows its content, empty state or error state. Later calls are ignored. */
  markReady = (): void => {
    try {
      if (this.readyAt !== null) return
      this.readyAt = this.now()
      this.sendReady()
    } catch {
      // Never let timing break the panel.
    }
  }

  private sendReady(): void {
    if (this.readyAt === null || this.name === null || this.readySent) return
    this.readySent = true
    this.send('ready', this.readyAt)
  }

  private send(stage: IslandStage, at: number): void {
    try {
      if (this.name === null || !timingSampled()) return
      if (currentNavigation().id !== this.navigation.id) return
      reportIslandTiming(this.name, stage, this.navigation, at)
    } catch {
      // Never let timing break the panel.
    }
  }
}

/** A timing handle for a panel to pass to its `TelemetryBoundary`. */
export function createIslandTiming(): IslandTimer {
  return new IslandTimer()
}
