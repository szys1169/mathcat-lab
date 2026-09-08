import path from "node:path";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";

const execFileAsync = promisify(execFile);

export async function revealInFileManager(target) {
  const resolved = path.resolve(target);
  if (process.platform === "darwin") {
    await execFileAsync("open", ["-R", resolved], { encoding: "utf8", timeout: 8_000 });
    return;
  }
  if (process.platform === "win32") {
    await execFileAsync("explorer.exe", ["/select,", resolved], { windowsHide: true, encoding: "utf8", timeout: 8_000 });
    return;
  }
  await execFileAsync("xdg-open", [path.dirname(resolved)], { encoding: "utf8", timeout: 8_000 });
}

export async function pickFolder() {
  if (process.platform === "darwin") {
    const script = 'POSIX path of (choose folder with prompt "选择 MathCat 工作区")';
    const result = await execFileAsync("osascript", ["-e", script], { encoding: "utf8", timeout: 120_000 });
    return result.stdout.trim().replace(/\/$/, "");
  }
  if (process.platform === "win32") {
    const script = fileURLToPath(new URL("../scripts/pick-folder.ps1", import.meta.url));
    const result = await execFileAsync("powershell.exe", ["-NoLogo", "-NoProfile", "-STA", "-ExecutionPolicy", "Bypass", "-File", script], { windowsHide: false, encoding: "utf8", timeout: 120_000 });
    return result.stdout.trim();
  }
  const result = await execFileAsync("zenity", ["--file-selection", "--directory", "--title=选择 MathCat 工作区"], { encoding: "utf8", timeout: 120_000 });
  return result.stdout.trim();
}
