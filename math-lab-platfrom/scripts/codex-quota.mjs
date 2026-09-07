import { spawn } from 'node:child_process';
import readline from 'node:readline';

export function weeklyWindow(result) {
  const bucket = result.rateLimitsByLimitId ? result.rateLimitsByLimitId.codex : result.rateLimits;
  const weekly = [bucket?.primary, bucket?.secondary].filter(w => w?.windowDurationMins === 10080);
  if (weekly.length !== 1 || !Number.isFinite(weekly[0].usedPercent) || !Number.isFinite(weekly[0].resetsAt)) throw new Error('Weekly Codex allowance unavailable');
  return weekly[0];
}

export function quotaDecision(window, policy, now = Date.now()) {
  if (policy.mode === 'until_user_stop') {
    if (!Number.isFinite(window.usedPercent) || window.usedPercent < 0 || window.usedPercent > 100 || !Number.isFinite(window.resetsAt) || now >= window.resetsAt * 1000) return 'quota_observation_invalid';
    return window.usedPercent >= 100 ? 'account_quota_exhausted' : null;
  }
  if (policy.mode && policy.mode !== 'bounded_weekly') return 'unknown_budget_mode';
  if (window.resetsAt !== policy.resetsAt || now >= policy.resetsAt * 1000) return 'weekly_window_changed';
  if (window.usedPercent < policy.baseline) return 'usage_counter_decreased';
  if (window.usedPercent >= policy.stopAt) return 'quota_reserve_reached';
  return null;
}

// An observation for the isolated worker, not an input to enforcement.
// Editing or deleting this receipt cannot authorize another model call.
export function quotaReceipt(window, policy, now = Date.now()) {
  const reason = quotaDecision(window, policy, now);
  return {
    schemaVersion: 1, projectId: policy.projectId, checkedAt: new Date(now).toISOString(),
    freshUntil: new Date(now + 90000).toISOString(),
    source: 'host_codex_account_rate_limits', enforcement: 'host_process_wrapper',
    allowed: reason === null, stopReason: reason, budgetMode: policy.mode || 'bounded_weekly',
    usedPercent: window.usedPercent, baselinePercent: policy.baseline,
    stopAtPercent: policy.stopAt, nominalCeilingPercent: policy.nominalCeiling,
    resetsAt: window.resetsAt,
    operatorAuthorization: policy.authorization || null,
    note: policy.mode === 'until_user_stop'
      ? '用户于2026-09-06明确授权持续研究直到用户停止，取代原25个百分点/28%停止规则；原基线仅作历史，不再限制运行。宿主每30秒查询额度，查询失败或账户额度耗尽时暂停，巡检确认恢复后可重试；不购买额度、不使用重置券、不改用付费API。跨自然周可继续。此回执不控制预算，任务无需另启额度客户端。'
      : '宿主在模型派发前及每30秒直接读取账户额度；阈值、接口失败、周窗口改变或停止标记触发进程取消。此文件仅提供任务可见的回执，不控制预算。任务无需自行启动另一个额度客户端。'
  };
}

export async function codexRead(binary, method, params) {
  return new Promise((resolve, reject) => {
    const child = spawn(binary, ['app-server'], { windowsHide: true, stdio: ['pipe', 'pipe', 'pipe'] });
    let done = false;
    const finish = (error, value) => { if (done) return; done = true; clearTimeout(timer); child.kill(); error ? reject(error) : resolve(value); };
    const timer = setTimeout(() => finish(new Error('Codex account read timed out')), 25000);
    child.on('error', e => finish(e));
    child.on('exit', code => { if (!done) finish(new Error(`Account reader exited: ${code}`)); });
    child.stderr.resume();
    child.stdin.on('error', e => finish(e));
    const send = data => child.stdin.write(JSON.stringify(data) + '\n');
    readline.createInterface({ input: child.stdout }).on('line', line => {
      let message; try { message = JSON.parse(line); } catch { return; }
      if (message.id === 1) {
        if (message.error) return finish(new Error(message.error.message));
        send({ method: 'initialized' });
        send({ id: 2, method, ...(params ? { params } : {}) });
      }
      if (message.id === 2) finish(message.error ? new Error(message.error.message) : null, message.result);
    });
    send({ id: 1, method: 'initialize', params: { clientInfo: { name: 'mathcat-quota-guard', version: '1.0.0' } } });
  });
}
