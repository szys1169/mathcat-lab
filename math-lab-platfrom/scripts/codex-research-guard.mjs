import fs from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawn } from 'node:child_process';
import { pipeline } from 'node:stream/promises';
import { authorizedPrompt, executionPolicyAddendum } from './codex-execution-policy.mjs';
import { codexRead, weeklyWindow, quotaDecision, quotaReceipt } from './codex-quota.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const bundledWindows = path.join(root, 'runtime-tools/codex/node_modules/@openai/codex-win32-x64/vendor/x86_64-pc-windows-msvc/bin/codex.exe');
const binary = process.env.CODEX_BIN || (process.platform === 'win32' ? bundledWindows : 'codex');
const args = process.argv.slice(2);
const cwdIndex = args.indexOf('--cd');
const cwd = path.resolve(cwdIndex >= 0 ? args[cwdIndex + 1] : process.cwd());
const policyFile = path.join(root, 'runtime-data/cu-research-policy.json');
const policy = JSON.parse(await fs.readFile(policyFile, 'utf8'));
const scoped = cwd.split(path.sep).includes(policy.projectId);
let child, timer, checking = false, stopping = false;
const log = async data => fs.appendFile(path.join(policy.workspace, 'quota-audit.jsonl'), JSON.stringify({ at: new Date().toISOString(), pid: process.pid, ...data }) + '\n');
const stoppedFile = path.join(policy.workspace, 'QUOTA-STOP.json');
const exists = async file => fs.stat(file).then(() => true, e => { if (e.code === 'ENOENT') return false; throw e; });

async function stopProject(reason) {
  const { config } = await import('../src/config.mjs');
  const { MathCatClient } = await import('../src/mathcat-client.mjs');
  const client = new MathCatClient(config.mathcat);
  for (let attempt = 0; attempt < 4; attempt++) {
    try {
      const board = await client.board(policy.projectId);
      if (['success','refuted','partial_success','stopped_by_human','environment_failed','error'].includes(board.status)) return;
      await client.stopProject(policy.projectId, board);
      return;
    } catch (e) { if (attempt === 3) throw e; }
  }
}

async function terminate(reason) {
  if (stopping) return;
  stopping = true;
  clearInterval(timer);
  await fs.writeFile(stoppedFile, JSON.stringify({ at: new Date().toISOString(), reason, policy }, null, 2), { flag: 'wx' }).catch(e => { if (e.code !== 'EEXIST') throw e; });
  await log({ event: 'stop', reason });
  // Kill only this wrapper's actual child tree, never a process discovered by name.
  if (child?.pid) {
    if (process.platform === 'win32') await new Promise(resolve => { const killer = spawn('taskkill.exe', ['/PID', String(child.pid), '/T', '/F'], { windowsHide: true, stdio: 'ignore' }); killer.on('error', resolve); killer.on('exit', resolve); });
    else { try { process.kill(-child.pid, 'SIGTERM'); } catch { child.kill('SIGTERM'); } }
  }
  await stopProject(reason).catch(e => log({event:'stop_command_error',error:e.message}));
  process.stderr.write(`MathCat budget guard stopped this research: ${reason}\n`);
  process.exit(75);
}

async function check() {
  if (checking || stopping) return;
  checking = true;
  try {
    if (await exists(stoppedFile)) return await terminate('persisted_stop');
    const w = weeklyWindow(await codexRead(binary, 'account/rateLimits/read'));
    await log({ event: 'quota', ...w, stopAt: policy.stopAt, nominalCeiling: policy.nominalCeiling });
    const reason = quotaDecision(w, policy);
    if (reason) await terminate(reason);
    const receiptFile = path.join(cwd, 'quota-status.json');
    const receiptTemp = `${receiptFile}.${process.pid}.tmp`;
    await fs.writeFile(receiptTemp, JSON.stringify(quotaReceipt(w, policy), null, 2));
    await fs.rename(receiptTemp, receiptFile);
  } catch (e) { await terminate(`quota_check_failed: ${e.message}`); }
  finally { checking = false; }
}

let effective = args;
if (scoped) {
  await fs.mkdir(cwd, { recursive: true });
  await check();
  effective = [];
  for (let i = 0; i < args.length; i++) {
    if (args[i] === '--model' || args[i] === '-m') { i++; continue; }
    if (args[i].startsWith('--model=')) continue;
    effective.push(args[i]);
  }
  // Global config options precede the exec/resume subcommands and cannot become prompt text.
  effective.unshift('-c', 'model="gpt-6-astra"', '-c', 'model_reasoning_effort="high"', '-c', 'approval_policy="never"');
  await fs.mkdir(path.join(cwd, 'sources'), {recursive:true});
  for (const name of ['Castro-Urbano-1994.pdf', 'Castro-Urbano-1994.txt']) {
    await fs.copyFile(path.join(policy.workspace, 'sources', name), path.join(cwd, 'sources', name));
  }
  await log({ event: 'dispatch', model: 'gpt-6-astra', reasoning: 'high', cwd });
  timer = setInterval(() => { void check(); }, 30000);
}
// MathCat passes the complete task prompt through `exec ... -` on stdin.
// Append the newer user-confirmed operational policy without rewriting history.
const attachPolicy = scoped && args.at(-1) === '-' && Boolean(executionPolicyAddendum(policy));
let promptFailed = false;
if (attachPolicy) await log({ event: 'execution_policy_attached', projectId: policy.projectId, mode: policy.mode });
child = spawn(binary, effective, { windowsHide: true, detached: process.platform !== 'win32', stdio: attachPolicy ? ['pipe', 'inherit', 'inherit'] : 'inherit' });
child.on('error', e => { clearInterval(timer); process.stderr.write(e.message + '\n'); process.exit(1); });
child.on('exit', (code) => { if (!stopping) { clearInterval(timer); process.exit(promptFailed ? 1 : (code ?? 1)); } });
if (attachPolicy) {
  void pipeline(authorizedPrompt(process.stdin, policy), child.stdin).catch(error => {
    promptFailed = true;
    process.stderr.write(`MathCat prompt forwarding failed: ${error.message}\n`);
  });
}
