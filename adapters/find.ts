import { existsSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

/**
 * The user's own `name` executable: from PATH, else from where installers
 * usually put it. GUI editors on macOS start agents with a minimal PATH that
 * misses ~/.local/bin and Homebrew.
 */
export function findAgent(name: string, extraDirs: string[] = []): string | undefined {
  const onPath = Bun.which(name);
  if (onPath) return onPath;
  const home = homedir();
  const dirs = [
    join(home, ".local/bin"),
    ...extraDirs.map((d) => d.replace(/^~(?=\/|$)/, home)),
    "/opt/homebrew/bin",
    "/usr/local/bin",
  ];
  return dirs.map((d) => join(d, name)).find((p) => existsSync(p));
}

/**
 * `--version`: the adapter's name, and the npm package and version it was
 * built from, then exit. brnr doctor reads it.
 */
export function version(adapter: string, pkg: { name: string; version: string }) {
  const arg = process.argv[2];
  if (arg === "--version" || arg === "-V") {
    console.log(`${adapter} ${pkg.version} (${pkg.name})`);
    process.exit(0);
  }
}

/** Points `envVar` at the user's `name`, or exits 127 like a shell would. */
export function requireAgent(adapter: string, name: string, envVar: string, extraDirs: string[] = []) {
  if (process.env[envVar]) return;
  const path = findAgent(name, extraDirs);
  if (!path) {
    console.error(`${adapter}: ${name} not found; install it or set ${envVar}`);
    process.exit(127);
  }
  process.env[envVar] = path;
}
