import fs from "node:fs/promises";
import { spawn } from "node:child_process";
import path from "node:path";

export async function runProcess({ command, args, cwd, stdoutFile, stderrFile, signal, timeoutMs = 2 * 60 * 60 * 1000 }) {
  await fs.mkdir(path.dirname(stdoutFile), { recursive: true });
  const out = await fs.open(stdoutFile, "a"); const err = await fs.open(stderrFile, "a");
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { cwd, windowsHide: true, stdio: ["ignore", out.fd, err.fd] });
    let timedOut = false; let aborted = false; let settled = false;
    const stopTree = () => {
      if (!child.pid) return;
      if (process.platform === "win32") spawn("taskkill.exe", ["/PID", String(child.pid), "/T", "/F"], { windowsHide: true, stdio: "ignore" });
      else child.kill("SIGTERM");
    };
    const onAbort = () => { aborted = true; stopTree(); };
    if (signal?.aborted) onAbort(); else signal?.addEventListener("abort", onAbort, { once: true });
    const timer = setTimeout(() => { timedOut = true; stopTree(); }, timeoutMs);
    const finish = async (fn, value) => { if (settled) return; settled = true; clearTimeout(timer); signal?.removeEventListener("abort", onAbort); await out.close(); await err.close(); fn(value); };
    child.on("error", (error) => finish(reject, error));
    child.on("exit", (code, exitSignal) => finish(resolve, { code, signal: exitSignal, timedOut, aborted, pid: child.pid }));
  });
}
