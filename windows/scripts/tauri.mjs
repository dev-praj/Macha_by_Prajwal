import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const cliPath = fileURLToPath(
  new URL("../node_modules/@tauri-apps/cli/tauri.js", import.meta.url),
);
const args = process.argv.slice(2);
const normalizePublicValue = (value) => value.trim().replace(/^(['"])(.*)\1$/, "$2");

// Developer-owned public Supabase config is read only for local builds. Values
// in the process environment win; never print these values to build logs.
const buildEnv = { ...process.env };
for (const name of ["SUPABASE_URL", "SUPABASE_ANON_KEY", "MACHA_API_URL"]) {
  if (buildEnv[name]) buildEnv[name] = normalizePublicValue(buildEnv[name]);
}
for (const path of [
  resolve(process.cwd(), "..", ".env"),
  resolve(process.cwd(), "..", "supabase", "keys-macha.env"),
  resolve(process.cwd(), "..", "supabase", "keys-macha.env.txt"),
]) {
  try {
    const envFile = readFileSync(path, "utf8");
    for (const line of envFile.split(/\r?\n/)) {
      const match = line.match(/^\s*(SUPABASE_URL|SUPABASE_ANON_KEY|MACHA_API_URL|MACHA_SUPABASE_URL|MACHA_SUPABASE_ANON_KEY)\s*=\s*(.*?)\s*$/);
      if (match) {
        const name = match[1].replace(/^MACHA_/, "");
        if (!buildEnv[name]) buildEnv[name] = normalizePublicValue(match[2]);
      }
    }
  } catch {
    // Try the next developer config location; values are never printed.
  }
}

if (args[0] === "dev") {
  args.splice(
    1,
    0,
    "--config",
    fileURLToPath(new URL("../src-tauri/tauri.dev.conf.json", import.meta.url)),
  );
}

const result = spawnSync(process.execPath, [cliPath, ...args], {
  stdio: "inherit",
  env: buildEnv,
});

if (result.error) {
  throw result.error;
}

process.exitCode = result.status ?? 1;
