/**
 * Compile the headless helper into a standalone executable (design 46 §2.1):
 *
 *   OPENUI_TELEMETRY_DISABLED=1 DO_NOT_TRACK=1 bun run build:helper [--target bun-linux-x64]
 *
 * Outputs (packages/apps/dist/):
 *   theuth-app-parser              stable name for local use
 *   theuth-app-parser-<hash>       immutable name for release manifests
 *   library-hash                the hash as one lowercase hex line (same as generated/library-hash.txt)
 *
 * Fails if the bundle would contain React or the component UI: the helper must stay core-only.
 */
import { copyFileSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const pkgDir = join(import.meta.dir, "..");
const distDir = join(pkgDir, "dist");
const hash = readFileSync(
  join(pkgDir, "generated", "library-hash.txt"),
  "utf8",
).trim();
if (!/^[0-9a-f]{64}$/.test(hash))
  throw new Error(
    `generated/library-hash.txt is not a sha256 hex: "${hash}" — run \`bun run generate\``,
  );

const targetIdx = process.argv.indexOf("--target");
const target = targetIdx !== -1 ? process.argv[targetIdx + 1] : undefined;

mkdirSync(distDir, { recursive: true });

// Bundle (not compile) first to verify the dependency closure; the compiled binary is opaque.
// Bun emits a `// node_modules/<pkg>/…` banner per bundled module; the generated JSON legitimately mentions the pins as strings.
const probe = await Bun.build({
  entrypoints: [join(pkgDir, "src", "helper.ts")],
  target: "bun",
  minify: false,
});
if (!probe.success)
  throw new AggregateError(probe.logs, "helper bundle failed");
const bundled = await probe.outputs[0]!.text();
const modules = new Set(
  [...bundled.matchAll(/^\/\/ node_modules\/((?:@[^/]+\/)?[^/\n]+)/gm)].map(
    (m) => m[1]!,
  ),
);
for (const forbidden of [
  "react",
  "react-dom",
  "@openuidev/react-ui",
  "@openuidev/react-lang",
  "@openuidev/react-headless",
  "zustand",
]) {
  if (modules.has(forbidden))
    throw new Error(
      `helper bundle must not contain ${forbidden}; bundled packages: ${[...modules].join(", ")}`,
    );
}

// Windows executables carry .exe (Bun appends it to a suffix-less --outfile anyway; name it so the copy below finds it).
const exe = (target ? target.startsWith("bun-windows") : process.platform === "win32") ? ".exe" : "";
const stable = join(distDir, `theuth-app-parser${exe}`);
const args = [
  "build",
  "--compile",
  "--minify",
  ...(target ? ["--target", target] : []),
  "--define",
  "process.env.NODE_ENV='production'",
  "--outfile",
  stable,
  join(pkgDir, "src", "helper.ts"),
];
const proc = Bun.spawnSync(["bun", ...args], {
  cwd: pkgDir,
  stdio: ["ignore", "inherit", "inherit"],
  env: { ...process.env, OPENUI_TELEMETRY_DISABLED: "1", DO_NOT_TRACK: "1" },
});
if (proc.exitCode !== 0) process.exit(proc.exitCode ?? 1);

const versioned = join(distDir, `theuth-app-parser-${hash}${exe}`);
copyFileSync(stable, versioned);
writeFileSync(join(distDir, "library-hash"), `${hash}\n`);
console.error(`built ${stable} and ${versioned}`);
