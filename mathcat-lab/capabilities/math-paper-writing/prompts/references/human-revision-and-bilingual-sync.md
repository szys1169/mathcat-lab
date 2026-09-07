# 人工修改台账、风格决策与双语同步

只在出现作者实质反馈、项目级风格决策或多个语言版本时读取本文件。模型自检写入 `revision_notes.md`；作者明确提出的要求写入 `human_revision_log.md`，两者不得混写。

## 人工修改台账

每条作者意见在修改前登记，修改和验证后更新状态：

| ID | Original request | Location | Class | Action | Status | Validation | Sync |
|---|---|---|---|---|---|---|---|
| R1 | 作者原始要求或忠实转述 | 节、标签、段落 | `generalizable` / `paper-specific` | 拟采取或已采取的修改 | `pending` / `applied` / `verified` / `rejected-with-reason` | semantic / build / visual / citation | n/a / awaiting / synced |

规则：

- 保留作者意图，不把作者意见改写成模型自己的问题发现。
- `generalizable` 表示可进入通用写作机制；作者姓名、具体章节号、专用符号和单篇论文删改属于 `paper-specific`。
- `applied` 只表示已改文件；只有完成相应语义、编译、视觉或引用检查后才能标记 `verified`。
- 若作者意见会改变数学陈述或需要新增证明，标记 `meaning-risk` 并停在交接状态，不自行执行。

## 风格决策

当项目存在多种术语、记号或结构选择时，输出 `writer/style_decisions.md`：

| Category | Chosen form | Rejected alternatives | Scope | Source |
|---|---|---|---|---|
| terminology / notation / voice / structure / formatting | 项目采用形式 | 易混淆或弃用形式 | global / section / language | author / venue / source |

典型情形：

- 多篇文献对同一对象使用不同记号；选择一种主记号并全局使用。
- 元组与分量、环与代数、绝对对象与相对对象需要明确区分。
- 翻译可能把两个不同对象压缩为同一术语。
- 目标期刊或作者已决定摘要、章节、定理标题或公式排版方式。

## 双语同步

先由用户指定修订主版本。主版本仍有 `pending` 或 `applied` 条目时，不同步另一版本；主版本确认后再按表同步：

| Result/label | Primary location | Secondary location | Statement and hypotheses | Numbering/labels | Citations | Terminology | Status |
|---|---|---|---|---|---|---|---|

同步不是逐句直译，至少核对：

- 定理、引理和定义的假设、量词、对象类型与结论强度；
- 章节、环境、公式编号及 `\label`/`\ref`/`\cite`；
- 术语表中的英文、中文、LaTeX 写法和首次定义；
- 摘要、引言、贡献声明、局限和计算证据角色；
- 一种语言中的删节是否在另一版本留下孤立引用或过程痕迹。

只有各项一致并通过构建检查后，条目才能标记 `synced`。
