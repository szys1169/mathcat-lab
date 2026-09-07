export const STATUS_LABELS = Object.freeze({
  created: "已创建",
  running: "研究中",
  proposed: "待确认",
  active: "进行中",
  paused: "已暂停",
  pruned: "已否决",
  pending: "待处理",
  queued: "排队中",
  approved: "已批准",
  rejected: "已否决",
  accepted: "已接受",
  open: "待处理",
  solved: "已解决",
  blocked: "被阻塞",
  needs_human_review: "等待人工决定",
  partial_success: "部分完成",
  success: "研究完成",
  refuted: "已找到反例",
  stopped_by_human: "已中止",
  verifying: "验证中",
  verified: "已验证",
  unverified: "未验证",
  unplanned: "尚未规划",
  failed: "已失败",
  error: "出错"
});

export function statusLabel(value) { return STATUS_LABELS[value] || value || "未知"; }
export function boardReviewMode(value) { return { automatic: "自动运行", balanced: "关键节点审核", strict: "全程审核" }[value] || "全程审核"; }
export function reviewModeLabel(value) { return { automatic: "自动", balanced: "关键审核", strict: "全程审核" }[value] || "全程审核"; }
