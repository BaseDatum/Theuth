/**
 * The package-local files a bundle was built from (review m5), read from the module banners
 * (`// src/check.ts`, `// generated/library.json`) of an unminified `Bun.build` output; `node_modules/…`
 * modules are covered by bun.lock and patches/* instead. Written as `<bundle>.inputs` (one path per line,
 * relative to packages/apps, sorted) next to the bundle and committed, so `scripts/apps-bundle-hash.sh`
 * hashes exactly a bundle's real inputs with nothing but shasum.
 */
import { existsSync } from "node:fs";
import { join } from "node:path";

export function localInputs(unminified: string, pkgDir: string): string[] {
  const paths = [...unminified.matchAll(/^\s*\/\/ ((?:src|generated)\/[^\n]+)$/gm)].map((m) => m[1]!.trim());
  const unique = [...new Set(paths)].sort();
  for (const p of unique) {
    if (!existsSync(join(pkgDir, p))) throw new Error(`bundle input ${p} is not a file under ${pkgDir}`);
  }
  if (unique.length === 0) throw new Error("no package-local bundle inputs found in the module banners");
  return unique;
}

export async function writeInputs(bundleFile: string, inputs: string[]): Promise<void> {
  await Bun.write(bundleFile.replace(/\.js$/, ".inputs"), `${inputs.join("\n")}\n`);
}
