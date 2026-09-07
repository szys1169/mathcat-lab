use std::collections::{HashMap, HashSet};

use research_domain::{
    BoardCapabilities, BoardClaim, BoardPlanningSuggestion, BoardResearchSettings, BoardRoute,
    BoardSummary, DomainEvent, GraphEdge, GraphNode, GraphProjection, ResearchBoardView,
};
use serde_json::{Value, json};
use sqlx::Row;

use crate::{SqliteStore, StorageError, StorageResult, event_from_row, rows};

fn attribute_text(attributes: &research_domain::Attributes, key: &str) -> String {
    attributes
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn attribute_strings(attributes: &research_domain::Attributes, key: &str) -> Vec<String> {
    attributes
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

#[derive(Debug, Clone, Copy, Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct BoardInclude {
    pub tasks: bool,
    pub workers: bool,
    pub verification: bool,
    pub artifacts: bool,
    pub graph: bool,
}

impl SqliteStore {
    pub async fn research_board(
        &self,
        project_id: &str,
        timeline_limit: i64,
        include: BoardInclude,
        mut capabilities: BoardCapabilities,
    ) -> StorageResult<ResearchBoardView> {
        let mut tx = self.read_pool().begin().await?;
        let project_row = sqlx::query("SELECT * FROM projects WHERE project_id=?")
            .bind(project_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| StorageError::NotFound {
                kind: "project",
                id: project_id.into(),
            })?;
        let project = rows::project(&project_row)?;
        capabilities.can_approve_route &= project_row.try_get::<bool, _>("human_route_approval")?;
        let revision = project.revision;
        let event_cursor: i64 =
            sqlx::query_scalar("SELECT COALESCE(MAX(cursor),0) FROM events WHERE project_id=?")
                .bind(project_id)
                .fetch_one(&mut *tx)
                .await?;

        let route_rows =
            sqlx::query("SELECT * FROM routes WHERE project_id=? ORDER BY priority DESC,route_id")
                .bind(project_id)
                .fetch_all(&mut *tx)
                .await?;
        let routes = route_rows
            .iter()
            .map(rows::route)
            .collect::<StorageResult<Vec<_>>>()?;
        let task_rows = sqlx::query(
            "SELECT * FROM tasks WHERE project_id=? ORDER BY round,priority DESC,task_id",
        )
        .bind(project_id)
        .fetch_all(&mut *tx)
        .await?;
        let all_tasks = task_rows
            .iter()
            .map(rows::task)
            .collect::<StorageResult<Vec<_>>>()?;
        let goal_rows =
            sqlx::query("SELECT * FROM goals WHERE project_id=? ORDER BY priority DESC,goal_id")
                .bind(project_id)
                .fetch_all(&mut *tx)
                .await?;
        let goals = goal_rows
            .iter()
            .map(rows::goal)
            .collect::<StorageResult<Vec<_>>>()?;
        let uncertainty_rows = sqlx::query(
            "SELECT * FROM uncertainties WHERE project_id=? ORDER BY created_at,uncertainty_id",
        )
        .bind(project_id)
        .fetch_all(&mut *tx)
        .await?;
        let uncertainties = uncertainty_rows
            .iter()
            .map(rows::uncertainty)
            .collect::<StorageResult<Vec<_>>>()?;
        let fact_rows =
            sqlx::query("SELECT * FROM facts WHERE project_id=? ORDER BY created_at,fact_id")
                .bind(project_id)
                .fetch_all(&mut *tx)
                .await?;
        let facts = fact_rows
            .iter()
            .map(rows::fact)
            .collect::<StorageResult<Vec<_>>>()?;
        let candidate_rows = sqlx::query(
            "SELECT * FROM candidates WHERE project_id=? ORDER BY created_at,candidate_id",
        )
        .bind(project_id)
        .fetch_all(&mut *tx)
        .await?;
        let candidates = candidate_rows
            .iter()
            .map(rows::candidate)
            .collect::<StorageResult<Vec<_>>>()?;
        let verification_rows = sqlx::query("SELECT * FROM verifications WHERE project_id=? ORDER BY COALESCE(completed_at,started_at),verification_id")
            .bind(project_id).fetch_all(&mut *tx).await?;
        let verifications = verification_rows
            .iter()
            .map(rows::verification)
            .collect::<StorageResult<Vec<_>>>()?;
        let verification_by_candidate = verifications
            .iter()
            .map(|verification| {
                (
                    verification.candidate_id.clone(),
                    verification.verification_id.clone(),
                )
            })
            .collect::<HashMap<_, _>>();

        let mut board_routes = Vec::with_capacity(routes.len());
        for (route, row) in routes.iter().zip(route_rows.iter()) {
            let route_tasks = all_tasks
                .iter()
                .filter(|task| task.route_id == route.route_id)
                .collect::<Vec<_>>();
            let completed = route_tasks
                .iter()
                .filter(|task| task.status.to_string() == "completed")
                .count();
            let progress = if route_tasks.is_empty() {
                0.0
            } else {
                let completed = u32::try_from(completed).unwrap_or(u32::MAX);
                let total = u32::try_from(route_tasks.len()).unwrap_or(u32::MAX);
                f64::from(completed) / f64::from(total)
            };
            let active_task_ids = route_tasks
                .iter()
                .filter(|task| is_active_task(&task.status.to_string()))
                .map(|task| task.task_id.clone())
                .collect();
            let blocking_uncertainty_ids = uncertainties
                .iter()
                .filter(|item| {
                    item.status.to_string() == "open"
                        && item.affects_route_ids.contains(&route.route_id)
                })
                .map(|item| item.uncertainty_id.clone())
                .collect();
            board_routes.push(BoardRoute {
                route_id: route.route_id.clone(),
                title: route.title.clone(),
                method_summary: route.method_summary.clone(),
                approach_kind: attribute_text(&route.attributes, "approach_kind"),
                route_role: attribute_text(&route.attributes, "route_role"),
                user_title: attribute_text(&route.attributes, "user_title"),
                plain_language_summary: attribute_text(&route.attributes, "plain_language_summary"),
                why_this_route: attribute_text(&route.attributes, "why_this_route"),
                expected_output: attribute_text(&route.attributes, "expected_output"),
                relation_to_goal: attribute_text(&route.attributes, "relation_to_goal"),
                steps: attribute_strings(&route.attributes, "steps"),
                status: route.status.to_string(),
                human_review: row.try_get("human_review")?,
                progress: progress.clamp(0.0, 1.0),
                target_goal_ids: route.target_goal_ids.clone(),
                active_task_ids,
                blocking_uncertainty_ids,
                updated_at: None,
            });
        }

        let mut claims = candidates
            .iter()
            .map(|candidate| BoardClaim {
                claim_id: candidate.candidate_id.clone(),
                kind: "candidate".into(),
                statement: candidate.submission.statement.clone(),
                status: if candidate.status.to_string() == "accepted" {
                    "promoted".into()
                } else {
                    candidate.status.to_string()
                },
                origin_route_id: Some(candidate.submission.route_id.clone()),
                verification_id: verification_by_candidate
                    .get(&candidate.candidate_id)
                    .cloned(),
                fact_id: None,
            })
            .collect::<Vec<_>>();
        claims.extend(facts.iter().map(|fact| BoardClaim {
            claim_id: fact.fact_id.clone(),
            kind: "fact".into(),
            statement: fact.statement.clone(),
            status: if fact.status.to_string() == "active" {
                "accepted".into()
            } else {
                fact.status.to_string()
            },
            origin_route_id: None,
            verification_id: fact.verification_ids.last().cloned(),
            fact_id: Some(fact.fact_id.clone()),
        }));
        let hypothesis_rows = sqlx::query(
            "SELECT * FROM hypotheses WHERE project_id=? ORDER BY created_in_round,hypothesis_id",
        )
        .bind(project_id)
        .fetch_all(&mut *tx)
        .await?;
        let hypotheses = hypothesis_rows
            .iter()
            .map(rows::hypothesis)
            .collect::<StorageResult<Vec<_>>>()?;
        claims.extend(hypotheses.into_iter().map(|hypothesis| BoardClaim {
            claim_id: hypothesis.hypothesis_id,
            kind: "hypothesis".into(),
            statement: hypothesis.statement,
            status: hypothesis.status,
            origin_route_id: hypothesis.route_id,
            verification_id: None,
            fact_id: hypothesis.promoted_to_fact_id,
        }));
        let source_rows =
            sqlx::query("SELECT * FROM sources WHERE project_id=? ORDER BY retrieved_at,source_id")
                .bind(project_id)
                .fetch_all(&mut *tx)
                .await?;
        let sources = source_rows
            .iter()
            .map(rows::source)
            .collect::<StorageResult<Vec<_>>>()?;
        claims.extend(sources.into_iter().map(|source| BoardClaim {
            claim_id: source.source_id,
            kind: "unverified_source".into(),
            statement: source.statement_excerpt.unwrap_or(source.title),
            status: "unverified".into(),
            origin_route_id: source.origin_route_id,
            verification_id: None,
            fact_id: None,
        }));

        let question_rows = sqlx::query(
            "SELECT * FROM human_questions WHERE project_id=? ORDER BY created_at,question_id",
        )
        .bind(project_id)
        .fetch_all(&mut *tx)
        .await?;
        let human_questions = question_rows
            .iter()
            .map(question_value)
            .collect::<StorageResult<Vec<_>>>()?;
        let planning_suggestion_rows = sqlx::query(
            "SELECT suggestion_id,content,target_route_id,status,decision,created_in_round,effective_round \
             FROM suggestions WHERE project_id=? ORDER BY created_in_round,suggestion_id",
        )
        .bind(project_id)
        .fetch_all(&mut *tx)
        .await?;
        let planning_suggestions = planning_suggestion_rows
            .iter()
            .map(|row| {
                let decision = row
                    .try_get::<Option<String>, _>("decision")?
                    .map(|raw| serde_json::from_str::<Value>(&raw))
                    .transpose()?;
                Ok(BoardPlanningSuggestion {
                    suggestion_id: row.try_get("suggestion_id")?,
                    content: row.try_get("content")?,
                    target_route_id: row.try_get("target_route_id")?,
                    status: row.try_get("status")?,
                    decision,
                    created_in_round: row.try_get("created_in_round")?,
                    effective_round: row.try_get("effective_round")?,
                })
            })
            .collect::<StorageResult<Vec<_>>>()?;
        let timeline_rows =
            sqlx::query("SELECT * FROM events WHERE project_id=? ORDER BY cursor DESC LIMIT ?")
                .bind(project_id)
                .bind(timeline_limit.clamp(0, 500))
                .fetch_all(&mut *tx)
                .await?;
        let mut timeline = timeline_rows
            .iter()
            .map(event_from_row)
            .collect::<StorageResult<Vec<DomainEvent>>>()?;
        timeline.reverse();

        let workers = if include.workers {
            let rows = sqlx::query("SELECT * FROM workers WHERE project_id=? ORDER BY worker_id")
                .bind(project_id)
                .fetch_all(&mut *tx)
                .await?;
            rows.iter()
                .map(rows::worker)
                .collect::<StorageResult<Vec<_>>>()?
        } else {
            vec![]
        };
        let artifacts = if include.artifacts {
            let rows =
                sqlx::query("SELECT * FROM artifacts WHERE project_id=? ORDER BY created_at DESC")
                    .bind(project_id)
                    .fetch_all(&mut *tx)
                    .await?;
            rows.iter()
                .map(rows::artifact)
                .collect::<StorageResult<Vec<_>>>()?
        } else {
            vec![]
        };
        let graph = if include.graph {
            Some(board_graph(&mut tx, project_id, revision).await?)
        } else {
            None
        };
        let failed_routes = board_routes
            .iter()
            .filter(|route| matches!(route.status.as_str(), "pruned" | "failed" | "human_stopped"))
            .cloned()
            .collect();
        let active_routes = board_routes
            .iter()
            .filter(|route| is_active_route(&route.status))
            .count();
        let summary = BoardSummary {
            current_round: project.current_round,
            active_routes,
            active_tasks: all_tasks
                .iter()
                .filter(|task| is_active_task(&task.status.to_string()))
                .count(),
            open_goals: goals
                .iter()
                .filter(|goal| goal.status.to_string() != "solved")
                .count(),
            accepted_facts: facts
                .iter()
                .filter(|fact| fact.status.to_string() == "active")
                .count(),
            pending_candidates: candidates
                .iter()
                .filter(|candidate| {
                    matches!(
                        candidate.status.to_string().as_str(),
                        "submitted" | "verifying" | "unknown"
                    )
                })
                .count(),
            blocking_uncertainties: uncertainties
                .iter()
                .filter(|item| item.status.to_string() == "open" && item.severity == "critical")
                .count(),
            pending_human_questions: human_questions
                .iter()
                .filter(|question| question.get("status").and_then(Value::as_str) == Some("open"))
                .count(),
        };
        tx.commit().await?;
        Ok(ResearchBoardView {
            schema_version: 1,
            project_id: project.project_id,
            agent: "mathcat".into(),
            mode: "human_collaboration".into(),
            status: project.status.to_string(),
            revision,
            event_cursor,
            problem: project.contract,
            settings: BoardResearchSettings {
                budget: project.budget,
                review_mode: project.review_mode,
                human_route_approval: project.human_route_approval,
                running_task_policy:
                    "settings apply to future task admission and newly created immutable task contracts; running attempts continue"
                        .into(),
            },
            summary,
            routes: board_routes
                .into_iter()
                .filter(|route| {
                    !matches!(route.status.as_str(), "pruned" | "failed" | "human_stopped")
                })
                .collect(),
            goals,
            claims,
            failed_routes,
            planning_suggestions,
            human_questions,
            uncertainties,
            tasks: if include.tasks { all_tasks } else { vec![] },
            workers,
            verification_queue: if include.verification {
                verifications
            } else {
                vec![]
            },
            artifacts,
            timeline,
            graph,
            capabilities,
        })
    }
}

fn is_active_task(status: &str) -> bool {
    matches!(
        status,
        "queued"
            | "offered"
            | "leased"
            | "running"
            | "checkpointed"
            | "result_submitted"
            | "ingesting"
    )
}

fn is_active_route(status: &str) -> bool {
    matches!(
        status,
        "incubating" | "active" | "blocked" | "probation" | "revived"
    )
}

fn question_value(row: &sqlx::sqlite::SqliteRow) -> StorageResult<Value> {
    Ok(json!({
        "question_id":row.try_get::<String,_>("question_id")?,
        "question":row.try_get::<String,_>("question")?,
        "options":serde_json::from_str::<Value>(row.try_get("options_json")?)?,
        "blocking_entity_ids":serde_json::from_str::<Value>(row.try_get("blocking_entity_ids_json")?)?,
        "status":row.try_get::<String,_>("status")?,
        "answer":row.try_get::<Option<String>,_>("answer_json")?.map(|raw| serde_json::from_str::<Value>(&raw)).transpose()?,
        "asked_by":row.try_get::<String,_>("asked_by")?,
        "answered_by":row.try_get::<Option<String>,_>("answered_by")?,
        "timeout_at":row.try_get::<Option<String>,_>("timeout_at")?,
        "created_at":row.try_get::<String,_>("created_at")?,
        "answered_at":row.try_get::<Option<String>,_>("answered_at")?,
    }))
}

async fn board_graph(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    project_id: &str,
    revision: i64,
) -> StorageResult<GraphProjection> {
    let mut nodes = Vec::new();
    for row in sqlx::query("SELECT goal_id,statement,status FROM goals WHERE project_id=? ORDER BY created_in_round,goal_id")
        .bind(project_id)
        .fetch_all(&mut **tx)
        .await?
    {
        nodes.push(GraphNode {
            id: row.try_get("goal_id")?,
            kind: "goal".into(),
            label: row.try_get("statement")?,
            status: row.try_get("status")?,
            attributes: Default::default(),
        });
    }
    for row in sqlx::query(
        "SELECT route_id,title,method_summary,status,human_review,attributes_json FROM routes WHERE project_id=? AND status NOT IN ('merged','pruned','failed','human_stopped') ORDER BY priority DESC,route_id",
    )
    .bind(project_id)
    .fetch_all(&mut **tx)
    .await?
    {
        let mut attributes: research_domain::Attributes =
            serde_json::from_str(row.try_get("attributes_json")?)?;
        let title: String = row.try_get("title")?;
        let method_summary: String = row.try_get("method_summary")?;
        let label = attribute_text(&attributes, "user_title");
        attributes.insert("technical_title".into(), json!(title.clone()));
        attributes.insert("technical_summary".into(), json!(method_summary));
        nodes.push(GraphNode {
            id: row.try_get("route_id")?,
            kind: "route".into(),
            label: if label.is_empty() { title } else { label },
            status: if row.try_get::<String, _>("human_review")? == "pending" {
                "pending".into()
            } else {
                row.try_get("status")?
            },
            attributes,
        });
    }
    for row in
        sqlx::query("SELECT hypothesis_id,kind,statement,status FROM hypotheses WHERE project_id=?")
            .bind(project_id)
            .fetch_all(&mut **tx)
            .await?
    {
        if row.try_get::<String, _>("kind")? == "route" {
            continue;
        }
        nodes.push(GraphNode {
            id: row.try_get("hypothesis_id")?,
            kind: row.try_get("kind")?,
            label: row.try_get("statement")?,
            status: row.try_get("status")?,
            attributes: Default::default(),
        });
    }
    for row in sqlx::query("SELECT fact_id,statement,status FROM facts WHERE project_id=?")
        .bind(project_id)
        .fetch_all(&mut **tx)
        .await?
    {
        nodes.push(GraphNode {
            id: row.try_get("fact_id")?,
            kind: "fact".into(),
            label: row.try_get("statement")?,
            status: row.try_get("status")?,
            attributes: Default::default(),
        });
    }
    let mut edges = Vec::new();
    for table in ["goal_edges", "hypothesis_edges", "fact_edges"] {
        let query =
            format!("SELECT edge_id,source_id,target_id,kind FROM {table} WHERE project_id=?");
        for row in sqlx::query(&query)
            .bind(project_id)
            .fetch_all(&mut **tx)
            .await?
        {
            edges.push(GraphEdge {
                id: row.try_get("edge_id")?,
                source: row.try_get("source_id")?,
                target: row.try_get("target_id")?,
                kind: row.try_get("kind")?,
            });
        }
    }
    let node_ids = nodes
        .iter()
        .map(|node| node.id.as_str())
        .collect::<HashSet<_>>();
    edges.retain(|edge| {
        node_ids.contains(edge.source.as_str()) && node_ids.contains(edge.target.as_str())
    });
    Ok(GraphProjection {
        graph_type: "combined".into(),
        revision,
        nodes,
        edges,
    })
}
