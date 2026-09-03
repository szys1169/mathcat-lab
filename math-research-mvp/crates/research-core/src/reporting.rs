use std::fmt::Write;

use research_domain::{PlannerOutput, Project, ProjectSnapshot};

pub fn initial_report(project: &Project) -> String {
    format!(
        "# {}\n\n- 项目 ID：`{}`\n- 状态：`{}`\n- 当前轮次：0\n- 主目标完成：否\n\n## Problem Contract\n\n{}\n\n## 当前可信结论\n\n尚无已验证事实。\n\n## 下一步\n\n通过 API 或 CLI 启动项目后，Planner 将生成至少两条研究路线。\n",
        project.name, project.project_id, project.status, project.contract.target_statement,
    )
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
    let _ = writeln!(output, "- 数据 revision：{}", snapshot.project_revision);
    let main_solved = snapshot
        .goals
        .iter()
        .any(|goal| goal.priority >= 1.0 && goal.status.to_string() == "solved");
    let _ = writeln!(
        output,
        "- 主目标完成：{}",
        if main_solved {
            "是（仍以最终闭包检查为准）"
        } else {
            "否"
        }
    );
    let _ = writeln!(output, "\n## 本轮规划摘要\n\n{}", plan.rationale_summary);
    output.push_str("\n## 已验证事实\n");
    if snapshot.facts.is_empty() {
        output.push_str("\n尚无。\n");
    }
    for fact in &snapshot.facts {
        let _ = writeln!(
            output,
            "\n- `{}` [{}]：{}",
            fact.fact_id, fact.evidence_level, fact.statement
        );
    }
    output.push_str("\n## 活跃研究路线\n");
    for route in &snapshot.routes {
        let _ = writeln!(
            output,
            "\n- `{}` [{}，score={:.3}] {} — {}",
            route.route_id, route.status, route.score, route.title, route.method_summary
        );
    }
    output.push_str("\n## Worker 与任务结果\n");
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
    output.push_str("\n## 信任说明\n\n只有上面的 active 已验证事实可以作为正式证明前提；路线、发现、草稿、计算观察和不确定性都不是数学事实。来源中只有 `admitted` 条目可进入论文引用；`reported_unverified` 仍只是检索线索。\n");
    output
}
