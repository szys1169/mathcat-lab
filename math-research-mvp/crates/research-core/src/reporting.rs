use std::fmt::Write;

use research_domain::{Fact, FactStatus, PlannerOutput, Project, ProjectSnapshot, ProjectStatus};

fn main_goal_completion_label(status: ProjectStatus) -> &'static str {
    match status {
        ProjectStatus::Success => "是（证明闭包已通过）",
        ProjectStatus::Refuted => "是（已由认证反例否证）",
        _ => "否",
    }
}

pub fn initial_report(project: &Project) -> String {
    format!(
        "# {}\n\n- 项目 ID：`{}`\n- 状态：`{}`\n- 当前轮次：0\n- 主目标完成：否\n\n## Problem Contract\n\n{}\n\n## 当前可信结论\n\n尚无已验证事实。\n\n## 下一步\n\n通过 API 或 CLI 启动项目后，Planner 将生成至少两条研究路线。\n",
        project.name, project.project_id, project.status, project.contract.target_statement,
    )
}

fn append_fact_sections(output: &mut String, facts: &[Fact]) {
    output.push_str("\n## 已验证事实\n");
    let mut active_facts = facts
        .iter()
        .filter(|fact| fact.status == FactStatus::ActiveFact)
        .peekable();
    if active_facts.peek().is_none() {
        output.push_str("\n尚无。\n");
    }
    for fact in active_facts {
        let _ = writeln!(
            output,
            "\n- `{}` [{}]：{}",
            fact.fact_id, fact.evidence_level, fact.statement
        );
    }
    let mut inactive_facts = facts
        .iter()
        .filter(|fact| fact.status != FactStatus::ActiveFact)
        .peekable();
    if inactive_facts.peek().is_some() {
        output.push_str("\n## 非活跃事实（不可作为当前证明前提）\n");
        for fact in inactive_facts {
            let _ = writeln!(
                output,
                "\n- `{}` [状态：{} / {}]：{}",
                fact.fact_id, fact.status, fact.evidence_level, fact.statement
            );
        }
    }
}

pub fn round_report(snapshot: &ProjectSnapshot, plan: &PlannerOutput) -> String {
    let mut output = String::new();
    let _ = writeln!(
        output,
        "# {} — 第 {} 轮",
        snapshot.project.name, snapshot.project.current_round
    );
    let _ = writeln!(output, "\n- 项目 ID：`{}`", snapshot.project.project_id);
    let _ = writeln!(output, "- 状态：`{}`", snapshot.project.status);
    if snapshot.project.status == ProjectStatus::PartialSuccess {
        output.push_str("- 状态说明：`partial_success` 是主目标未闭合的执行终态，不保证已取得数学进展；以已验证事实和目标闭合记录为准。\n");
    }
    let _ = writeln!(output, "- 数据 revision：{}", snapshot.project_revision);
    let _ = writeln!(
        output,
        "- 主目标完成：{}",
        main_goal_completion_label(snapshot.project.status)
    );
    let _ = writeln!(output, "\n## 本轮规划摘要\n\n{}", plan.rationale_summary);
    append_fact_sections(&mut output, &snapshot.facts);
    output.push_str("\n## 活跃研究路线\n");
    for route in &snapshot.routes {
        let _ = writeln!(
            output,
            "\n- `{}` [{}，score={:.3}] {} — {}",
            route.route_id, route.status, route.score, route.title, route.method_summary
        );
    }
    output.push_str("\n## Worker 与任务结果\n");
    if snapshot.tasks.is_empty() {
        output.push_str("\n尚无研究任务。规划输出不等于已执行研究或已取得数学成果。\n");
    }
    for task in &snapshot.tasks {
        let _ = writeln!(
            output,
            "\n- `{}` [{} / {}] {}",
            task.task_id,
            task.worker_role,
            task.status,
            task.result_summary.as_deref().unwrap_or("尚无摘要")
        );
    }
    output.push_str("\n## 关键不确定性\n");
    if snapshot.uncertainties.is_empty() {
        output.push_str("\n尚无。\n");
    }
    for uncertainty in &snapshot.uncertainties {
        let _ = writeln!(
            output,
            "\n- `{}` [{} / {}] {}",
            uncertainty.uncertainty_id,
            uncertainty.severity,
            uncertainty.status,
            uncertainty.description
        );
    }
    output.push_str("\n## 文献来源台账\n");
    if snapshot.sources.is_empty() {
        output.push_str("\n尚无。\n");
    }
    for source in &snapshot.sources {
        let _ = writeln!(
            output,
            "\n- `{}` [{}] {} — {}",
            source.source_id, source.status, source.title, source.applicability
        );
    }
    output.push_str("\n## 信任说明\n\n只有上面的 active 已验证事实可以作为正式证明前提；路线、发现、草稿、计算观察和不确定性都不是数学事实。`lead_unverified` 只是待取全文的检索线索，`reported_unverified` 虽已有冻结材料仍未通过引用裁决；来源中只有 `admitted` 条目可进入论文引用。\n");
    output
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use research_domain::{
        Budget, Fact, FactStatus, PlannerOutput, ProblemContract, Project, ProjectSnapshot,
        ProjectStatus, ReviewMode,
    };

    use super::{main_goal_completion_label, round_report};

    fn report_with_fact_statuses(statuses: &[FactStatus]) -> String {
        let now = Utc::now();
        let snapshot = ProjectSnapshot {
            project: Project {
                project_id: "project-1".into(),
                name: "Fact status report".into(),
                contract: ProblemContract {
                    original_problem: "Prove P".into(),
                    target_statement: "P".into(),
                    assumptions: vec![],
                    success_criteria: "A certified proof or counterexample".into(),
                    version: 1,
                },
                status: ProjectStatus::PartialSuccess,
                revision: 1,
                current_round: 1,
                budget: Budget::default(),
                review_mode: ReviewMode::Automatic,
                human_route_approval: false,
                created_at: now,
                updated_at: now,
            },
            current_round: None,
            workers: vec![],
            tasks: vec![],
            routes: vec![],
            goals: vec![],
            hypotheses: vec![],
            facts: statuses
                .iter()
                .enumerate()
                .map(|(index, &status)| Fact {
                    fact_id: format!("fact-{index}"),
                    project_id: "project-1".into(),
                    statement: format!("statement-{status}-{index}"),
                    assumptions: vec![],
                    proof_markdown: "Recorded proof".into(),
                    dependency_fact_ids: vec![],
                    definitions_introduced: std::collections::BTreeMap::default(),
                    external_source_ids: vec![],
                    verification_ids: vec![],
                    evidence_level: "independent_llm_check".into(),
                    created_by: "verifier".into(),
                    status,
                    content_hash: "fixture".into(),
                    created_at: now,
                })
                .collect(),
            uncertainties: vec![],
            sources: vec![],
            project_revision: 1,
            event_cursor: 1,
        };
        let plan = PlannerOutput {
            rationale_summary: "The goal remains open.".into(),
            routes: vec![],
            assignments: vec![],
            targeted_uncertainty_ids: vec![],
            suggestion_decisions: vec![],
        };
        round_report(&snapshot, &plan)
    }

    #[test]
    fn mixed_fact_statuses_are_separated_in_report() {
        let report = report_with_fact_statuses(&[
            FactStatus::ActiveFact,
            FactStatus::Challenged,
            FactStatus::Suspended,
            FactStatus::Revoked,
        ]);
        let (trusted, inactive) = report
            .split_once("## 非活跃事实（不可作为当前证明前提）")
            .expect("separate non-active facts");
        assert!(trusted.contains("statement-active-0"));
        assert!(!inactive.contains("statement-active-0"));
        for (index, status) in [(1, "challenged"), (2, "suspended"), (3, "revoked")] {
            let statement = format!("statement-{status}-{index}");
            assert!(!trusted.contains(&statement));
            assert!(inactive.contains(&statement));
            assert!(inactive.contains(&format!("`fact-{index}` [状态：{status} /")));
        }
        assert!(report.contains("不保证已取得数学进展"));
        assert!(report.contains("尚无研究任务"));
    }

    #[test]
    fn entirely_non_active_facts_leave_trusted_section_empty() {
        let report = report_with_fact_statuses(&[
            FactStatus::Challenged,
            FactStatus::Suspended,
            FactStatus::Revoked,
        ]);
        let (trusted, inactive) = report
            .split_once("## 非活跃事实（不可作为当前证明前提）")
            .expect("non-active fact history remains available");
        assert!(trusted.ends_with("## 已验证事实\n\n尚无。\n\n"));
        assert!(!trusted.contains("`fact-"));
        for (index, status) in ["challenged", "suspended", "revoked"].iter().enumerate() {
            assert!(inactive.contains(&format!("statement-{status}-{index}")));
            assert!(inactive.contains(&format!("`fact-{index}` [状态：{status} /")));
        }
    }

    #[test]
    fn entirely_active_facts_need_no_non_active_section() {
        let report = report_with_fact_statuses(&[FactStatus::ActiveFact, FactStatus::ActiveFact]);
        let trusted = report
            .split_once("## 已验证事实\n")
            .expect("trusted facts section")
            .1
            .split_once("## 活跃研究路线")
            .expect("end of facts section")
            .0;
        assert!(trusted.contains("statement-active-0"));
        assert!(trusted.contains("statement-active-1"));
        assert!(!trusted.contains("尚无"));
        assert!(!report.contains("## 非活跃事实"));
    }

    #[test]
    fn completion_label_uses_authoritative_project_terminal_state() {
        assert_eq!(
            main_goal_completion_label(ProjectStatus::Success),
            "是（证明闭包已通过）"
        );
        assert_eq!(
            main_goal_completion_label(ProjectStatus::Refuted),
            "是（已由认证反例否证）"
        );
        for status in [
            ProjectStatus::Created,
            ProjectStatus::Running,
            ProjectStatus::Paused,
            ProjectStatus::PartialSuccess,
            ProjectStatus::EnvironmentFailed,
            ProjectStatus::NeedsHumanReview,
            ProjectStatus::StoppedByHuman,
            ProjectStatus::Error,
        ] {
            assert_eq!(main_goal_completion_label(status), "否");
        }
    }
}
