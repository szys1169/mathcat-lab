//! Read views over the same authoritative project aggregate used by the scheduler.
use super::{
    ApiResult, ApiState, Deserialize, HeaderMap, IntoResponse, Json, Path, Query, State,
    StatusCode, Value, entity, envelope, fields, idempotency, invalid, json, rows,
};

#[derive(Deserialize, Default)]
pub(super) struct RecordQuery {
    run_id: Option<String>,
    session_id: Option<String>,
    state: Option<String>,
}

async fn records(
    state: ApiState,
    project_id: String,
    query: RecordQuery,
    collection: &str,
) -> ApiResult<Json<Value>> {
    let project = state.service.store().read(&project_id).await?;
    if let Some(run_id) = &query.run_id {
        entity(&project, "runs", run_id)?;
    }
    if let Some(session_id) = &query.session_id {
        entity(&project, "sessions", session_id)?;
    }
    let values: Vec<Value> = rows(&project, collection)
        .iter()
        .filter(|item| {
            query.run_id.as_ref().is_none_or(|id| item["run_id"] == *id)
                && query.session_id.as_ref().is_none_or(|id| {
                    [
                        "session_id",
                        "owner_session_id",
                        "recipient_session_id",
                        "target_session_id",
                    ]
                    .iter()
                    .any(|key| item[*key] == *id)
                })
                && query
                    .state
                    .as_ref()
                    .is_none_or(|value| item["state"] == *value || item["status"] == *value)
        })
        .cloned()
        .collect();
    let mut response = envelope(collection, json!(values)).0;
    response["revision"] = project["revision"].clone();
    response["event_cursor"] = project["event_cursor"].clone();
    Ok(Json(response))
}

macro_rules! collection_handler {
    ($name:ident) => {
        pub(super) async fn $name(
            State(state): State<ApiState>,
            Path(project): Path<String>,
            Query(query): Query<RecordQuery>,
        ) -> ApiResult<Json<Value>> {
            records(state, project, query, stringify!($name)).await
        }
    };
}

collection_handler!(cycles);
collection_handler!(routes);
collection_handler!(messages);
collection_handler!(advisories);
collection_handler!(memory_entries);
collection_handler!(display_summaries);
collection_handler!(background_jobs);
collection_handler!(proof_checkpoints);
collection_handler!(pending_assignments);

#[derive(Deserialize, Default)]
pub(super) struct MemoryQuery {
    q: Option<String>,
    query: Option<String>,
    mode: Option<String>,
}

pub(super) async fn search_memories(
    State(state): State<ApiState>,
    Path(project): Path<String>,
    Query(query): Query<MemoryQuery>,
) -> ApiResult<Json<Value>> {
    let term = query.q.or(query.query).unwrap_or_default();
    if term.len() > 4096 {
        return Err(invalid("搜索词不能超过 4096 字节。"));
    }
    let mode = query.mode.as_deref().unwrap_or("experience");
    let records = state.service.memory_search22(&project, &term, mode).await?;
    let mut response = envelope("records", json!(records)).0;
    response["mode"] = json!(mode);
    Ok(Json(response))
}

pub(super) async fn read_statement(
    State(state): State<ApiState>,
    Path((project, statement)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    let value = state.service.read_statement22(&project, &statement).await?;
    Ok(envelope("statement", value))
}

pub(super) async fn update_limits(
    State(state): State<ApiState>,
    Path((project, run)): Path<(String, String)>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> ApiResult<Json<Value>> {
    let key = idempotency(&headers)?;
    // Validation and the control/version transaction belong to the service.
    let result = state
        .service
        .update_limits(&project, &run, &key, &input)
        .await?;
    Ok(envelope("run", result))
}

pub(super) async fn feedback(
    State(state): State<ApiState>,
    Path(project): Path<String>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> ApiResult<impl IntoResponse> {
    let key = idempotency(&headers)?;
    fields(
        &input,
        &[
            "run_id",
            "priority",
            "kind",
            "summary",
            "body",
            "evidence_refs",
            "affected_task_or_claim_ids",
            "target_session_id",
            "target",
            "recipient_session_id",
            "urgency_reason",
            "suggested_response",
            "expected_versions",
            "annotation_id",
            "annotation_revision",
            "selection_ref",
        ],
    )?;
    // Authenticated local human only. There is no externally supplied role or author.
    let result = state.service.feedback(&project, &key, &input).await?;
    Ok((StatusCode::ACCEPTED, envelope("command", result)))
}
