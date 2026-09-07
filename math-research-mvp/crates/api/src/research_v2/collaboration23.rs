use super::{
    ApiResult, ApiState, Deserialize, HeaderMap, Json, Path, Query, State, Value, envelope, fields,
    idempotency, rows,
};

#[derive(Deserialize, Default)]
pub(super) struct RunQuery {
    run_id: Option<String>,
}

pub(super) async fn feedback_traces(
    State(state): State<ApiState>,
    Path(project): Path<String>,
    Query(query): Query<RunQuery>,
) -> ApiResult<Json<Value>> {
    let traces = state
        .service
        .feedback_traces23(&project, query.run_id.as_deref())
        .await?;
    Ok(envelope("feedback_traces", super::json!(traces)))
}

pub(super) async fn questions(
    State(state): State<ApiState>,
    Path(project): Path<String>,
    Query(query): Query<RunQuery>,
) -> ApiResult<Json<Value>> {
    let project = state.service.store().read(&project).await?;
    let rows: Vec<Value> = rows(&project, "human_questions")
        .iter()
        .filter(|q| query.run_id.as_ref().is_none_or(|r| q["run_id"] == *r))
        .cloned()
        .collect();
    Ok(envelope("human_questions", super::json!(rows)))
}

async fn change(
    state: ApiState,
    project: String,
    command: String,
    headers: HeaderMap,
    input: Value,
    operation: &str,
) -> ApiResult<Json<Value>> {
    let key = idempotency(&headers)?;
    fields(&input, &["expected_revision", "reason"])?;
    Ok(envelope(
        "command",
        state
            .service
            .change_feedback23(&project, &command, operation, &input, &key)
            .await?,
    ))
}

pub(super) async fn withdraw(
    State(state): State<ApiState>,
    Path((project, command)): Path<(String, String)>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> ApiResult<Json<Value>> {
    change(state, project, command, headers, input, "withdraw").await
}

pub(super) async fn escalate(
    State(state): State<ApiState>,
    Path((project, command)): Path<(String, String)>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> ApiResult<Json<Value>> {
    change(state, project, command, headers, input, "escalate").await
}

pub(super) async fn answer(
    State(state): State<ApiState>,
    Path((project, question)): Path<(String, String)>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> ApiResult<Json<Value>> {
    let key = idempotency(&headers)?;
    fields(
        &input,
        &["run_id", "expected_revision", "body", "expected_versions"],
    )?;
    Ok(envelope(
        "command",
        state
            .service
            .answer_question23(&project, &question, &input, &key)
            .await?,
    ))
}

pub(super) async fn proof_tree(
    State(state): State<ApiState>,
    Path(project): Path<String>,
) -> ApiResult<Json<Value>> {
    Ok(envelope(
        "proof_tree",
        state.service.proof_tree23(&project).await?,
    ))
}
