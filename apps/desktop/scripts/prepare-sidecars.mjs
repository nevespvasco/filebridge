// `pnpm bundle` runs this before `tauri build` (beforeBuildCommand in
// src-tauri/tauri.bundle.conf.json): sidecars/prepare-desktop.sh, or prepare-desktop.ps1 on
// Windows, which may have no bash. Both read the same environment (RCLONE_DOWNLOAD, RCLONE, …).
import { spawnSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const sidecars = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..", "sidecars");
const [command, args] =
  process.platform === "win32"
    ? ["powershell.exe", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", join(sidecars, "prepare-desktop.ps1")]]
    : ["bash", [join(sidecars, "prepare-desktop.sh")]];
const run = spawnSync(command, args, { stdio: "inherit" });
if (run.error) {
  console.error(`could not run ${command}: ${run.error.message}`);
  process.exit(1);
}
process.exit(run.status ?? 1);
