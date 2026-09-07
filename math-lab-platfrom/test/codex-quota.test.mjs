import test from 'node:test';
import assert from 'node:assert/strict';
import { weeklyWindow, quotaDecision, quotaReceipt } from '../scripts/codex-quota.mjs';
const policy = { baseline: 6, stopAt: 28, resetsAt: 2000000000 };
test('explicit continuous authorization supersedes old cap and supports natural weekly rollover', () => {
  const continuous = {...policy,mode:'until_user_stop'};
  assert.equal(quotaDecision({usedPercent:44,resetsAt:policy.resetsAt},continuous,0),null);
  assert.equal(quotaDecision({usedPercent:0,resetsAt:policy.resetsAt+604800},continuous,policy.resetsAt*1000),null);
  assert.equal(quotaDecision({usedPercent:100,resetsAt:policy.resetsAt},continuous,0),'account_quota_exhausted');
  assert.equal(quotaReceipt({usedPercent:44,resetsAt:policy.resetsAt},continuous,0).budgetMode,'until_user_stop');
});
test('continuous mode still fails closed on invalid observations and unknown policies', () => {
  const continuous = {...policy,mode:'until_user_stop'};
  for (const usedPercent of [NaN,Infinity,-1,101]) assert.equal(quotaDecision({usedPercent,resetsAt:policy.resetsAt},continuous,0),'quota_observation_invalid');
  assert.equal(quotaDecision({usedPercent:44,resetsAt:policy.resetsAt},continuous,policy.resetsAt*1000),'quota_observation_invalid');
  assert.equal(quotaDecision({usedPercent:44,resetsAt:policy.resetsAt},{...policy,mode:'typo'},0),'unknown_budget_mode');
});
test('quota selects weekly window in either slot and ignores Spark', () => {
  const w = { usedPercent: 6, windowDurationMins: 10080, resetsAt: policy.resetsAt };
  assert.equal(weeklyWindow({rateLimitsByLimitId:{codex:{secondary:w},codex_bengalfox:{primary:{...w,usedPercent:99}}}}), w);
  assert.equal(weeklyWindow({rateLimits:{primary:w}}),w);
});
test('missing, ambiguous or malformed quota fails closed', () => {
  for (const value of [{}, {rateLimitsByLimitId:{}}, {rateLimits:{primary:{windowDurationMins:10080,usedPercent:null}}}]) assert.throws(()=>weeklyWindow(value));
});
test('quota stops before nominal 31 percent ceiling', () => {
  assert.equal(quotaDecision({usedPercent:27,resetsAt:policy.resetsAt},policy,0),null);
  assert.equal(quotaDecision({usedPercent:28,resetsAt:policy.resetsAt},policy,0),'quota_reserve_reached');
  assert.equal(quotaDecision({usedPercent:32,resetsAt:policy.resetsAt},policy,0),'quota_reserve_reached');
});
test('reset, counter decrease and elapsed week cannot reopen the budget', () => {
  assert.equal(quotaDecision({usedPercent:0,resetsAt:policy.resetsAt+1},policy,0),'weekly_window_changed');
  assert.equal(quotaDecision({usedPercent:5,resetsAt:policy.resetsAt},policy,0),'usage_counter_decreased');
  assert.equal(quotaDecision({usedPercent:6,resetsAt:policy.resetsAt},policy,policy.resetsAt*1000),'weekly_window_changed');
});
test('task-visible receipt has bounded freshness and no account secrets', () => {
  const r = quotaReceipt({ usedPercent: 8, resetsAt: policy.resetsAt }, {...policy, projectId:'p1', nominalCeiling:31}, 1000);
  assert.equal(r.allowed,true);
  assert.equal(r.projectId,'p1');
  assert.equal(Date.parse(r.freshUntil)-Date.parse(r.checkedAt),90000);
  assert.equal(r.usedPercent,8);
  assert.equal(r.enforcement,'host_process_wrapper');
  assert.equal('token' in r,false);
});
test('receipt cannot turn a stopped decision into permission', () => {
  const r = quotaReceipt({usedPercent:28,resetsAt:policy.resetsAt},policy,0);
  assert.equal(r.allowed,false);
  assert.equal(r.stopReason,'quota_reserve_reached');
  r.allowed=true;
  assert.equal(quotaDecision({usedPercent:28,resetsAt:policy.resetsAt},policy,0),'quota_reserve_reached');
});
