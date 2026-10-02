// Failed-login throttle: at most `max` failures per `windowMs` per client key, in memory.

export class AttemptLimiter {
  private failures = new Map<string, number[]>();
  private readonly max: number;
  private readonly windowMs: number;

  constructor(max = 5, windowMs = 15 * 60 * 1000) {
    this.max = max;
    this.windowMs = windowMs;
  }

  private recent(key: string, now: number): number[] {
    const list = (this.failures.get(key) ?? []).filter((t) => now - t < this.windowMs);
    if (list.length) this.failures.set(key, list);
    else this.failures.delete(key);
    return list;
  }

  /** Seconds the client must wait, or 0 when it may try. */
  retryAfterSecs(key: string, now: number = Date.now()): number {
    const list = this.recent(key, now);
    if (list.length < this.max) return 0;
    return Math.max(1, Math.ceil((list[0] + this.windowMs - now) / 1000));
  }

  fail(key: string, now: number = Date.now()): void {
    const list = this.recent(key, now);
    list.push(now);
    this.failures.set(key, list);
    // Bound memory under a spray of distinct keys.
    if (this.failures.size > 5000) this.failures.clear();
  }

  success(key: string): void {
    this.failures.delete(key);
  }
}
