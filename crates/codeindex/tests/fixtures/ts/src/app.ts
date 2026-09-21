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

