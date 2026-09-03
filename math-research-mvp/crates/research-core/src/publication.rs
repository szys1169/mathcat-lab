use std::collections::{BTreeSet, HashSet};

use research_domain::ProjectSnapshot;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaperWriterOutput {
    pub status: String,
    pub article_plan_md: String,
    pub claim_evidence_ledger_md: String,
    pub related_work_tex: String,
    pub article_candidate_tex: String,
    pub revision_notes_md: String,
    pub evidence_gaps_md: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicationResult {
    pub publication_id: String,
    pub project_id: String,
    pub status: String,
    pub output_directory: String,
    pub evidence_gaps: Vec<String>,
    pub artifact_ids: Vec<String>,
    pub cited_keys: Vec<String>,
}

pub(crate) fn source_packet(snapshot: &ProjectSnapshot) -> Value {
    let facts = snapshot
        .facts
        .iter()
        .filter(|fact| fact.status.to_string() == "active")
        .collect::<Vec<_>>();
    let sources = snapshot
        .sources
        .iter()
        .filter(|source| source.status == "admitted")
        .collect::<Vec<_>>();
    let uncertainties = snapshot
        .uncertainties
        .iter()
        .filter(|item| matches!(item.status.to_string().as_str(), "open" | "investigating"))
        .collect::<Vec<_>>();
    json!({
        "packet_version": 1,
        "project_id": snapshot.project.project_id,
        "problem_contract": snapshot.project.contract,
        "goals": snapshot.goals,
        "active_facts": facts,
        "admitted_sources": sources,
        "open_uncertainties": uncertainties,
        "trust_contract": {
            "mathematical_claims": "active_facts_only",
            "citations": "admitted_sources_only",
            "writer_may_modify_facts": false,
            "writer_may_search_or_prove": false
        }
    })
}

pub(crate) fn publication_gaps(snapshot: &ProjectSnapshot, allow_partial: bool) -> Vec<String> {
    let active_facts = snapshot
        .facts
        .iter()
        .filter(|fact| fact.status.to_string() == "active")
        .collect::<Vec<_>>();
    let admitted = snapshot
        .sources
        .iter()
        .filter(|source| source.status == "admitted")
        .map(|source| (source.source_id.as_str(), source))
        .collect::<std::collections::HashMap<_, _>>();
    let mut gaps = Vec::new();
    if active_facts.is_empty() {
        gaps.push("没有 active Fact，论文写作层无可消费的已验证数学结果。".into());
    }
    if !allow_partial {
        let main_goals = snapshot
            .goals
            .iter()
            .filter(|goal| goal.priority >= 1.0)
            .collect::<Vec<_>>();
        let main_closed = !main_goals.is_empty()
            && main_goals
                .iter()
                .all(|goal| matches!(goal.status.to_string().as_str(), "solved" | "refuted"));
        if !main_closed {
            gaps.push(
                "主目标尚未 solved/refuted；使用 --allow-partial 只能生成明确标注的阶段稿。".into(),
            );
        }
        for uncertainty in snapshot.uncertainties.iter().filter(|item| {
            matches!(item.status.to_string().as_str(), "open" | "investigating")
                && matches!(item.severity.as_str(), "high" | "critical")
        }) {
            gaps.push(format!(
                "存在阻断写作的 {} 不确定性：{}",
                uncertainty.severity, uncertainty.description
            ));
        }
        if admitted.is_empty() {
            gaps.push("没有 admitted Source，无法生成带可审计 Related Work 的投稿式论文。".into());
        }
    }
    for fact in active_facts {
        for source_id in &fact.external_source_ids {
            match admitted.get(source_id.as_str()) {
                None => gaps.push(format!(
                    "Fact {} 引用了尚未 admitted 的来源 {}。",
                    fact.fact_id, source_id
                )),
                Some(source) if source.citation_key.as_deref().is_none_or(str::is_empty) => gaps
                    .push(format!(
                        "admitted 来源 {source_id} 缺少 citation_key，不能安全写入引用。"
                    )),
                Some(_) => {}
            }
        }
    }
    gaps.sort();
    gaps.dedup();
    gaps
}

pub(crate) fn writer_prompt(
    packet: &Value,
    allow_partial: bool,
) -> Result<String, serde_json::Error> {
    Ok(format!(
        r"你是数学论文写作器，不是证明器、验证器或文献搜索器。只把 Source Packet 中已经完成的数学结果组织成候选论文。

硬约束：
1. 只能把 active_facts 写成数学主张；保持每个定理/引理的陈述、量词和假设，不得加强、削弱或补造结论。
2. 只能引用 admitted_sources，且只能使用其中给出的 citation_key。不得根据记忆补引用，不得生成不存在的 BibTeX。
3. Related Work 只陈述来源包能支持的历史、比较和归属；按可证据化的历史时间线、最接近结果对比、本文贡献定位组织，并为每项陈述绑定 citation_key。证据不足就写入 evidence_gaps_md。
4. claim_evidence_ledger_md 必须逐项包含论文实际使用的 fact_id，并说明对应章节、依赖 Fact、验证记录和外部来源。可以不使用与本文无关的 active Fact，但论文中的每个数学主张都必须映射到至少一个 active Fact。
5. article_candidate_tex 与 related_work_tex 不得出现内部 fact_id/task_id/project_id、运行路径、智能体协作记录或“系统已验证”等内部流程描述。
6. 计算结果必须明确标成计算证据，不得冒充一般证明。不要搜索文献、寻找新证明或改变数学内容。
7. allow_partial={allow_partial}。阶段稿必须在题目、摘要和结论中明确其覆盖范围，不能伪装成已完成主目标。
8. ready 稿必须是可独立编译的完整 LaTeX 文档。每个使用的 citation_key 都必须在 article_candidate_tex 的 thebibliography 环境中有对应的 \bibitem{{citation_key}}；不得依赖外部 .bib、图片或其他文件。
9. 若材料不足，status=evidence_gaps，article_candidate_tex 可为空；否则 status=ready。只返回给定 JSON Schema。

Source Packet：
{}",
        serde_json::to_string_pretty(packet)?
    ))
}

#[allow(clippy::too_many_lines)]
pub(crate) fn validate_writer_output(
    output: &PaperWriterOutput,
    packet: &Value,
) -> Result<Vec<String>, String> {
    if output.status != "ready" && output.status != "evidence_gaps" {
        return Err(format!("unsupported writer status {}", output.status));
    }
    if output.status == "evidence_gaps" {
        if output.evidence_gaps_md.trim().is_empty() {
            return Err("writer returned evidence_gaps without describing the gaps".into());
        }
        return Ok(Vec::new());
    }
    if output.article_candidate_tex.trim().is_empty() {
        return Err("ready writer output has an empty article_candidate_tex".into());
    }
    let unsafe_tex = [
        "\\write18",
        "\\input",
        "\\include",
        "\\includegraphics",
        "\\lstinputlisting",
        "\\verbatiminput",
        "\\bibliography",
        "\\addbibresource",
        "\\openin",
        "\\openout",
        "\\newread",
        "\\newwrite",
        "\\read",
        "\\immediate",
        "\\catcode",
        "\\csname",
        "\\filecontents",
        "\\usepackage{shellesc}",
        "^^",
    ];
    let normalized_tex = output
        .article_candidate_tex
        .to_ascii_lowercase()
        .chars()
        .filter(|character| !character.is_ascii_whitespace())
        .collect::<String>();
    if let Some(command) = unsafe_tex
        .iter()
        .find(|command| normalized_tex.contains(**command))
    {
        return Err(format!("manuscript contains unsafe TeX command {command}"));
    }
    let forbidden = [
        "fact_",
        "task_",
        "project_",
        "runtime/",
        "runtime\\",
        "output/",
    ];
    let public_text = format!(
        "{}\n{}",
        output.article_candidate_tex, output.related_work_tex
    )
    .to_ascii_lowercase();
    if let Some(marker) = forbidden
        .iter()
        .find(|marker| public_text.contains(**marker))
    {
        return Err(format!("public manuscript leaks internal marker {marker}"));
    }
    let allowed_fact_ids = packet["active_facts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|fact| fact["fact_id"].as_str())
        .collect::<HashSet<_>>();
    let cited_fact_ids = extract_internal_ids(&output.claim_evidence_ledger_md, "fact_");
    if cited_fact_ids.is_empty() {
        return Err(
            "claim-evidence ledger does not map any manuscript claim to an active Fact".into(),
        );
    }
    if let Some(fact_id) = cited_fact_ids
        .iter()
        .find(|fact_id| !allowed_fact_ids.contains(fact_id.as_str()))
    {
        return Err(format!(
            "claim-evidence ledger references non-active fact {fact_id}"
        ));
    }
    let allowed_keys = packet["admitted_sources"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|source| source["citation_key"].as_str())
        .collect::<HashSet<_>>();
    let mut cited = extract_citation_keys(&format!(
        "{}\n{}",
        output.article_candidate_tex, output.related_work_tex
    ));
    if let Some(key) = cited
        .iter()
        .find(|key| !allowed_keys.contains(key.as_str()))
    {
        return Err(format!("manuscript cites non-admitted key {key}"));
    }
    cited.sort();
    for key in &cited {
        let bibitem = format!("\\bibitem{{{key}}}");
        if !output.article_candidate_tex.contains(&bibitem) {
            return Err(format!(
                "cited key {key} has no self-contained \\bibitem in article_candidate_tex"
            ));
        }
    }
    Ok(cited)
}

fn extract_internal_ids(text: &str, prefix: &str) -> BTreeSet<String> {
    text.split(|character: char| {
        !(character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
    })
    .filter(|token| token.starts_with(prefix) && token.len() > prefix.len())
    .map(ToOwned::to_owned)
    .collect()
}

fn extract_citation_keys(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut index = 0;
    let mut keys = BTreeSet::new();
    while index + 5 < bytes.len() {
        if &bytes[index..index + 5] != b"\\cite" {
            index += 1;
            continue;
        }
        let mut cursor = index + 5;
        while cursor < bytes.len() && bytes[cursor].is_ascii_alphabetic() {
            cursor += 1;
        }
        while cursor < bytes.len() && bytes[cursor] == b'[' {
            if let Some(offset) = bytes[cursor + 1..].iter().position(|byte| *byte == b']') {
                cursor += offset + 2;
            } else {
                break;
            }
        }
        if cursor >= bytes.len() || bytes[cursor] != b'{' {
            index = cursor.saturating_add(1);
            continue;
        }
        let start = cursor + 1;
        if let Some(offset) = bytes[start..].iter().position(|byte| *byte == b'}') {
            let end = start + offset;
            if let Ok(contents) = std::str::from_utf8(&bytes[start..end]) {
                keys.extend(
                    contents
                        .split(',')
                        .map(str::trim)
                        .filter(|key| !key.is_empty())
                        .map(ToOwned::to_owned),
                );
            }
            index = end + 1;
        } else {
            break;
        }
    }
    keys.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{PaperWriterOutput, extract_citation_keys, validate_writer_output};

    #[test]
    fn extracts_common_latex_citation_forms() {
        assert_eq!(
            extract_citation_keys(r"A \cite{alpha,beta}; B \citet[Thm. 2]{gamma}."),
            vec!["alpha", "beta", "gamma"]
        );
    }

    #[test]
    fn rejects_citations_outside_the_admitted_whitelist() {
        let packet = json!({
            "active_facts":[{"fact_id":"fact_allowed"}],
            "admitted_sources":[{"citation_key":"Allowed2026"}]
        });
        let output = PaperWriterOutput {
            status: "ready".into(),
            article_plan_md: "plan".into(),
            claim_evidence_ledger_md: "fact_allowed -> theorem 1".into(),
            related_work_tex: r"Prior work \cite{Invented2026}.".into(),
            article_candidate_tex: "\\section{Result}".into(),
            revision_notes_md: String::new(),
            evidence_gaps_md: String::new(),
        };
        assert_eq!(
            validate_writer_output(&output, &packet),
            Err("manuscript cites non-admitted key Invented2026".into())
        );
    }

    #[test]
    fn rejects_internal_identifiers_in_public_manuscript() {
        let packet = json!({
            "active_facts":[{"fact_id":"fact_allowed"}],
            "admitted_sources":[]
        });
        let output = PaperWriterOutput {
            status: "ready".into(),
            article_plan_md: "plan".into(),
            claim_evidence_ledger_md: "fact_allowed -> theorem 1".into(),
            related_work_tex: String::new(),
            article_candidate_tex: "Internal fact_allowed was accepted.".into(),
            revision_notes_md: String::new(),
            evidence_gaps_md: String::new(),
        };
        assert!(
            validate_writer_output(&output, &packet)
                .expect_err("internal id must fail")
                .contains("fact_")
        );
    }

    #[test]
    fn writer_may_select_only_relevant_active_facts() {
        let packet = json!({
            "active_facts":[{"fact_id":"fact_used"},{"fact_id":"fact_unrelated"}],
            "admitted_sources":[]
        });
        let output = PaperWriterOutput {
            status: "ready".into(),
            article_plan_md: "plan".into(),
            claim_evidence_ledger_md: "Main theorem -> fact_used".into(),
            related_work_tex: String::new(),
            article_candidate_tex: "\\section{Result} A proved theorem.".into(),
            revision_notes_md: String::new(),
            evidence_gaps_md: String::new(),
        };
        assert_eq!(validate_writer_output(&output, &packet), Ok(Vec::new()));
    }

    #[test]
    fn rejects_tex_that_can_read_or_execute_external_content() {
        let packet = json!({
            "active_facts":[{"fact_id":"fact_used"}],
            "admitted_sources":[]
        });
        let output = PaperWriterOutput {
            status: "ready".into(),
            article_plan_md: "plan".into(),
            claim_evidence_ledger_md: "Claim -> fact_used".into(),
            related_work_tex: String::new(),
            article_candidate_tex: "\\documentclass{article}\\input{../../secret}".into(),
            revision_notes_md: String::new(),
            evidence_gaps_md: String::new(),
        };
        assert!(
            validate_writer_output(&output, &packet)
                .expect_err("unsafe TeX must fail")
                .contains("unsafe TeX")
        );
    }
}
