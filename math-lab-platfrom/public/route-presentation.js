const text = (value, fallback = "") => String(value ?? fallback).trim();
const list = (value) => Array.isArray(value) ? value : [];

export const RESEARCH_LANES = Object.freeze({
  direct_proof: { label: "直接证明", description: "从定义、结构或已知定理出发，尝试闭合证明链。", order: 10 },
  counterexample: { label: "寻找反例", description: "攻击量词、边界和最小对象，寻找可核验的反例。", order: 20 },
  computation: { label: "计算探索", description: "用可重放计算寻找模式、候选见证或反例。", order: 30 },
  reduction: { label: "归约与推广", description: "从已知情形、特殊类、切片或递推机制推广。", order: 40 },
  literature: { label: "文献与已知技巧", description: "核验可用定理和来源，寻找能真正适用的工具。", order: 50 },
  formalization: { label: "问题准备", description: "补齐命题、定义与量词；它是前置工作，不是证明。", order: 60 },
  other: { label: "其它技巧", description: "尚未归入标准方向的具体数学尝试。", order: 70 }
});

const VALID_KINDS = new Set(Object.keys(RESEARCH_LANES));
const VALID_ROLES = new Set(["primary", "adversarial", "auxiliary", "prerequisite"]);

function plainJargon(value) {
  return text(value)
    .replace(/量词规格化/g, "明确量词与对象范围")
    .replace(/定义依赖闭包(?:恢复)?/g, "补齐命题所需定义")
    .replace(/可实例化性(?:反压力)?(?:双向)?审计/g, "检查定义能否用于具体对象")
    .replace(/中央桥梁/g, "关键引理")
    .replace(/[“”"]/g, "")
    .replace(/\s+/g, " ")
    .trim();
}

function routeTitle(route) {
  return `${text(route?.userTitle || route?.user_title)} ${text(route?.title)} ${text(route?.technicalTitle)}`.trim();
}

function actionableSummary(route) {
  const sentences = text(route?.plainLanguageSummary || route?.plain_language_summary || route?.summary || route?.method_summary)
    .split(/(?<=[。！？；.!?])/u)
    .map((item) => item.trim())
    .filter(Boolean)
    .filter((item) => !/^(?:即使|就算|哪怕|若只|如果只|禁止|不得|不能|不可|不声称|风险|注意|仍(?:然)?|尚(?:未|需)|但(?:是)?|然而)/.test(item));
  return sentences.slice(0, 3).join(" ");
}

function inferKind(value, { title = false } = {}) {
  if (!value) return "";
  if (/问题准备|形式化|量词|定义闭包|语义|规格|命题恢复|准入|解释唯一性/.test(value)) return "formalization";
  if (/文献|出处|原文|来源|定理工具箱|bibliograph|literature/i.test(value)) return "literature";
  if (/Artinian|对偶|socle|Koszul|Rees|valuation|赋值|syzygy|导子|乘法核|统一非零|直接证明|从定义|构造.*(?:证明|见证)|结构定理/i.test(value)) return "direct_proof";
  if (/计算|实验|程序|Groebner|Gröbner|Singular|Macaulay|Sage|census|穷举/i.test(value)) return "computation";
  if ((title && /反例|证伪|否定命题|边界攻击/.test(value)) || /(?:寻找|搜索|构造|枚举|验证|排除|攻击).{0,16}(?:反例|失败对象)|(?:反例|失败对象).{0,12}(?:搜索|构造|候选)/.test(value)) return "counterexample";
  if (/归纳|递推|归约|切片|变形|推广|特殊类|分类|gluing|悬挂|Thom.?Sebastiani/i.test(value)) return "reduction";
  return "";
}

export function routeApproachKind(route, board = {}) {
  const explicit = text(route?.approachKind || route?.approach_kind).toLowerCase();
  if (VALID_KINDS.has(explicit)) return explicit;
  const titleKind = inferKind(routeTitle(route), { title: true });
  if (titleKind) return titleKind;
  const tasks = list(board.tasks).filter((task) => text(task.route_id || task.routeId) === text(route?.id || route?.route_id));
  if (tasks.some((task) => task.worker_role === "counterexample_hunter")) return "counterexample";
  if (tasks.some((task) => task.strategic_role === "computation")) return "computation";
  return inferKind(actionableSummary(route)) || "other";
}

export function routeRole(route) {
  const explicit = text(route?.routeRole || route?.route_role).toLowerCase();
  if (VALID_ROLES.has(explicit)) return explicit;
  const title = routeTitle(route);
  if (/前置|问题准备|命题恢复|先查清|先确定/.test(title)) return "prerequisite";
  if (/辅助|加强版|部分推进/.test(title)) return "auxiliary";
  if (/反例|证伪|边界攻击/.test(title)) return "adversarial";
  const action = actionableSummary(route);
  if (/作为前置|先补齐|先恢复/.test(action)) return "prerequisite";
  if (/作为辅助|辅助路线|只推进.*子目标/.test(action)) return "auxiliary";
  if (/(?:寻找|搜索|构造|排除).{0,12}反例/.test(action)) return "adversarial";
  return "primary";
}

const ROLE_LABELS = Object.freeze({ primary: "主路线", adversarial: "反向检验", auxiliary: "辅助路线", prerequisite: "前置工作" });

function techniqueSummary(kind, title) {
  if (/Artinian|对偶|socle|乘法核/i.test(title)) return "把问题转到有限维 Artinian 商代数，研究关键元素的乘法映射之核与 socle，再把得到的见证翻译回原命题。";
  if (/Rees|valuation|赋值|积分闭包/i.test(title)) return "用控制积分闭包的 Rees 赋值比较候选元素与目标理想，寻找能被逐项核验的不包含见证。";
  if (/syzygy|协同|导子|Euler/i.test(title)) return "分析生成元之间的 syzygy 与 Euler 导子，尝试从显式关系中构造满足目标条件的元素。";
  if (/归纳|递推/.test(title)) return "按对象的自然规模建立基例和归纳步，并检查归纳假设没有悄悄加强原命题。";
  if (/切片|悬挂|变形|特殊类|拟齐次|quasihomogeneous/i.test(title)) return "先解决结构较清楚的特殊类，再寻找保持全部假设的切片、悬挂或变形机制推广到一般情形。";
  return "";
}

function fallbackSteps(kind, title) {
  if (/Artinian|对偶|socle|乘法核/i.test(title)) return ["把原命题翻译为商代数中的乘法核问题", "分析乘法核与 socle，构造候选见证", "将见证翻译回原对象并逐项核验结论"];
  if (/Rees|valuation|赋值|积分闭包/i.test(title)) return ["确定控制目标理想积分闭包的赋值", "构造并筛选可能违反包含关系的元素", "比较赋值条件并核验候选是否真正满足原命题"];
  const steps = {
    direct_proof: ["把目标改写为可操作的中间命题", "证明连接目标的关键引理", "检查引理是否在原假设下完整推出结论"],
    counterexample: ["确定最小对象与危险边界", "系统生成候选反例", "逐项核验假设和失败结论"],
    computation: ["确定可枚举的对象族与范围", "运行并保存可重放计算", "分析模式并交给证明或反例路线核验"],
    reduction: ["选择已经理解的特殊情形", "构造保持假设的归约或推广映射", "证明该映射覆盖原问题的全部对象"],
    literature: ["定位原始定理与精确出处", "核对全部假设和符号", "证明定理确实适用于当前目标"],
    formalization: ["补齐命题中的对象、量词和定义", "分清主目标与加强版或辅助目标", "形成可直接进入证明与反例搜索的陈述"],
    other: ["明确这条技巧操作的数学对象", "产出一个可检查的中间结果", "判断结果能否继续推进原目标"]
  };
  return steps[kind] || steps.other;
}

function fallbackCopy(kind, title) {
  const copies = {
    direct_proof: [
      `围绕“${title}”从定义和结构出发，尝试建立通向主结论的证明链。`,
      "这是当前最接近正面闭合目标的数学机制。",
      "一组可独立核验的引理，以及它们如何推出目标的完整连接。",
      "只有证明链真正到达原命题，这条路线才算完成。"
    ],
    counterexample: [
      `围绕“${title}”系统检查最小对象、边界情形和失败条件。`,
      "反例搜索能尽早判断命题是否错误，并暴露正面证明必须排除的危险结构。",
      "一个经过逐项核验的反例，或一份明确排除某类反例的结果。",
      "有效反例可直接否定目标；有限范围内没有反例不能证明目标。"
    ],
    computation: [
      `把“${title}”变成可重放的有限计算，记录输入、程序和输出。`,
      "计算适合发现模式和候选对象，也能为证明与反例路线提供方向。",
      "可复现脚本、输入范围、输出工件和对结果边界的说明。",
      "计算证据只支持下一步判断，除非同时给出覆盖所有情形的证明。"
    ],
    reduction: [
      `围绕“${title}”研究如何从已知情形、特殊类或较小对象推广。`,
      "归约可以把一般问题连接到已经理解的结构。",
      "一个保持全部假设的归约或推广引理，以及适用范围。",
      "只有归约覆盖原问题的全部对象时，才能闭合主目标。"
    ],
    literature: [
      `核验“${title}”需要的原始来源、定理陈述和适用条件。`,
      "先确认已有工具真实可用，可以避免重复证明或误用结果。",
      "带精确出处、完整假设和适用性判断的定理工具清单。",
      "文献结论只是可用输入，仍需明确说明它如何推进当前目标。"
    ],
    formalization: [
      `先完成“${title}”所需的命题、定义和量词澄清。`,
      "当前输入还不足以安全进入数学证明或反例搜索。",
      "一份无歧义、可逐项核对的完整问题陈述。",
      "这是证明前的准备工作，本身不算解决目标。"
    ],
    other: [
      `推进“${title}”所描述的具体数学技巧。`,
      "这是智能体针对当前卡点提出的候选方向。",
      "可检查的中间结果，以及继续或停止这条路线的依据。",
      "中间结果必须明确连接到原目标，才算实质推进。"
    ]
  };
  return copies[kind] || copies.other;
}

function readableTitle(route, kind) {
  const explicit = text(route?.userTitle || route?.user_title);
  if (explicit) return explicit;
  // Legacy MathCat boards are mapped with an empty researcher-facing `title` and keep the
  // original planner title in `technicalTitle`.  Treat an empty string as missing so those
  // persisted routes still receive a useful human-facing title.
  const original = text(route?.title) || text(route?.technicalTitle || route?.technical_title) || "未命名路线";
  if (/Artinian|socle|乘法核/i.test(original)) return "直接证明：用 Artinian 对偶研究乘法核";
  if (/Rees|valuation|积分闭包/i.test(original)) return "加强版：用 Rees 赋值检验积分闭包";
  const normalized = plainJargon(original);
  if (normalized.length <= 46 && !/^(?:即使|若只|禁止|不得|不能|风险)/.test(normalized)) return normalized;
  const afterMethod = normalized.match(/(?:它)?通过(.+?)(?:[。；]|$)/)?.[1];
  const action = plainJargon(afterMethod || actionableSummary(route) || normalized)
    .replace(/^(?:方法是|采用|使用|尝试)/, "")
    .split(/[。；]/)[0]
    .trim();
  const concise = action.length > 30 ? `${action.slice(0, 30)}…` : action;
  return `${RESEARCH_LANES[kind]?.label || "研究路线"}：${concise || "推进当前数学卡点"}`;
}

export function routePresentation(route, board = {}) {
  const kind = routeApproachKind(route, board);
  const role = routeRole(route);
  const title = readableTitle(route, kind);
  const fallback = fallbackCopy(kind, title);
  const steps = list(route?.steps).map((item) => text(item)).filter(Boolean);
  return {
    kind,
    role,
    lane: RESEARCH_LANES[kind],
    roleLabel: ROLE_LABELS[role],
    title,
    technicalTitle: text(route?.technicalTitle || route?.technical_title || route?.title),
    what: text(route?.plainLanguageSummary || route?.plain_language_summary) || techniqueSummary(kind, title) || fallback[0],
    why: text(route?.whyThisRoute || route?.why_this_route || route?.rationale) || fallback[1],
    deliverable: text(route?.expectedOutput || route?.expected_output || route?.deliverable) || fallback[2],
    relation: text(route?.relationToGoal || route?.relation_to_goal) || fallback[3],
    steps: steps.length ? steps : fallbackSteps(kind, title)
  };
}

export function expectedResearchLaneKinds(board = {}) {
  const value = text(board?.problem?.statement || board?.problem?.goal || board?.problem?.original);
  if (/prove\s+or\s+disprove|证明或(?:反驳|否定)|开放问题|open problem|猜想|conjecture/i.test(value)) {
    return ["direct_proof", "counterexample", "computation", "reduction"];
  }
  if (/证明|prove|theorem|定理/i.test(value)) return ["direct_proof", "reduction"];
  return [];
}

export function groupResearchRoutes(routes, board = {}, { includeExpected = false } = {}) {
  const groups = new Map();
  list(routes).forEach((route) => {
    const presentation = routePresentation(route, board);
    if (!groups.has(presentation.kind)) groups.set(presentation.kind, []);
    groups.get(presentation.kind).push({ route, presentation });
  });
  if (includeExpected) expectedResearchLaneKinds(board).forEach((kind) => { if (!groups.has(kind)) groups.set(kind, []); });
  return [...groups.entries()]
    .map(([kind, items]) => ({ kind, ...RESEARCH_LANES[kind], items, planned: items.length > 0 }))
    .sort((left, right) => left.order - right.order);
}
