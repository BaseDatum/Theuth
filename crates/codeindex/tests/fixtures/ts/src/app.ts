import { readFile } from "node:fs/promises";

export interface Config {
  path: string;
}

/** The loader. */
export class Loader {
  private cache: Record<string, string> = {};

  constructor(private cfg: Config) {}

  async load(name: string): Promise<string> {
    const hit = this.cache[name];
    if (hit) return hit;
    const text = await readFile(this.cfg.path, "utf8");
    return text;
  }
}

export function apply(f: (x: number) => number, x: number): number {
  // `f` is a parameter: filtered as a local.
  return f(x) + helper(x);
}

const helper = (x: number): number => x + 1;

export const VERSION = "1";


export function settle(): Promise<void> {
  // Destructured bindings are locals too: `reject(..)` is not a reference.
  const { promise, resolve, reject } = Promise.withResolvers<void>();
  const [first, second] = [resolve, reject];
  setTimeout(() => (Math.random() > 0.5 ? first() : second(new Error("no"))), 1);
  reject(new Error("never"));
  return promise;
}
