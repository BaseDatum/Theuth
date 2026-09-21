import { fmt } from "@lib/util";
import * as util from "@lib/util";
import { Loader, apply } from "./app";
import type { Config } from "./app";
import { readFile } from "node:fs/promises";
import "./missing-side-effect";

export async function main(cfg: Config): Promise<string> {
  const l = new Loader(cfg);
  const text = await l.load("x");
  const n = util.helper2(apply((v) => v + 1, 2));
  await readFile(cfg.path);
  return fmt(text) + n;
}
