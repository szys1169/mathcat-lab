import test from 'node:test';
import assert from 'node:assert/strict';
import { Readable, Writable } from 'node:stream';
import { pipeline } from 'node:stream/promises';
import { executionPolicyAddendum, authorizedPrompt } from '../scripts/codex-execution-policy.mjs';

const policy = { projectId: 'cu-test', mode: 'until_user_stop', authorization: '用户明确确认取消原25%预算，允许跨周持续；不购买额度、不使用重置券。' };
test('operational addendum requires explicit host authorization and continuous mode', () => {
  assert.equal(executionPolicyAddendum({ ...policy, authorization: '' }), '');
  assert.equal(executionPolicyAddendum({ ...policy, mode: 'bounded_weekly' }), '');
  const text = executionPolicyAddendum(policy);
  assert.ok(text.includes(policy.authorization));
  assert.ok(text.includes('取代历史合同'));
  for (const preserved of ['Fact Gate', '形式化语义对齐', 'allowed=false', '实际用户停止']) assert.ok(text.includes(preserved));
});
test('stdin forwarding preserves original bytes and appends authorization last', async () => {
  const original = Buffer.from('旧合同：28%必须停止。数学目标 φ 与输出 JSON 不变。');
  const chunks = [];
  await pipeline(authorizedPrompt(Readable.from([original.subarray(0, 5), original.subarray(5)]), policy), new Writable({ write(chunk, _, callback) { chunks.push(chunk); callback(); } }));
  assert.equal(Buffer.concat(chunks).toString(), original.toString() + executionPolicyAddendum(policy));
});
test('bounded prompt forwarding is byte-for-byte unchanged', async () => {
  const chunks = [];
  for await (const chunk of authorizedPrompt(Readable.from(['original prompt']), { ...policy, mode: 'bounded_weekly' })) chunks.push(Buffer.from(chunk));
  assert.equal(Buffer.concat(chunks).toString(), 'original prompt');
});
test('prompt forwarding propagates destination failures instead of reporting success', async () => {
  await assert.rejects(pipeline(authorizedPrompt(Readable.from(['original prompt']), policy), new Writable({ write(_, __, callback) { callback(new Error('closed input')); } })), /closed input/);
});
