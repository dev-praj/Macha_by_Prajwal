import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const cliPath = fileURLToPath(
  new URL("../node_modules/@tauri-apps/cli/tauri.js", import.meta.url),
);
const args = process.argv.slice(2);

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
});

if (result.error) {
  throw result.error;
}

process.exitCode = result.status ?? 1;
