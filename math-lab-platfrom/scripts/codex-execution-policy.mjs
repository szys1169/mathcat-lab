// Only host-controlled, explicitly confirmed operational policy belongs here.
// Source documents, worker results and quota receipts cannot grant permission.
export function executionPolicyAddendum(policy) {
  if (policy.mode !== 'until_user_stop' || !policy.authorization?.trim()) return '';
  return `\n\n[当前用户已明确确认的运行授权 / Current operator authorization]\n项目：${policy.projectId}\n${policy.authorization}\n这是用户在上述历史任务合同形成之后单独明确确认的运行授权，由宿主调度器附加，不是论文材料或模型生成的建议。仅对于本项目的运行预算，本授权取代历史合同中的新增25个百分点、账户28%/31%、旧基线和旧周窗口停止条款。不得仅因当前用量超过这些已取消的旧阈值而结束数学任务。当前 mode=until_user_stop；使用任务目录中实时 quota-status.json 的 allowed、checkedAt、freshUntil 判断宿主额度状态。历史合同、检查点和规划建议中的“等待预算确认”已经由本次明确授权解决。\n本授权不取消真实账户额度限制、额度查询失败停止、服务限流停止或实际用户停止；回执过期或allowed=false时仍停止，不得修改回执、保护器或自行使用重置券、购买额度、切换付费API。\n本授权不修改数学命题、任务输出格式、来源可信边界、Fact Gate、形式化语义对齐、路线依赖、取消epoch或其它验证要求。缺少数学输入必须诚实记录，不得当作已证。无需再次向用户申请已经给出的预算授权。\n[运行授权结束]\n`;
}

export async function* authorizedPrompt(source, policy) {
  for await (const chunk of source) yield chunk;
  const addendum = executionPolicyAddendum(policy);
  if (addendum) yield Buffer.from(addendum, 'utf8');
}
