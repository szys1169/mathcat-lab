use super::{
    ApiResult, ApiState, HeaderMap, Json, Path, State, Value, envelope, fields, idempotency,
};

pub(super) async fn problem_spec(
    State(state): State<ApiState>,
    Path(project): Path<String>,
) -> ApiResult<Json<Value>> {
    Ok(envelope(
        "problem_spec",
        state.service.problem_spec24(&project).await?,
    ))
}

pub(super) async fn update_problem_spec(
    State(state): State<ApiState>,
    Path(project): Path<String>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> ApiResult<Json<Value>> {
    fields(
        &input,
        &[
            "expected_revision",
            "math_statement",
            "research_description",
            "source",
        ],
    )?;
    Ok(envelope(
        "problem_spec",
        state
            .service
            .update_problem_spec24(&project, &input, &idempotency(&headers)?)
            .await?,
    ))
}

pub(super) async fn plans(
    State(state): State<ApiState>,
    Path(project): Path<String>,
) -> ApiResult<Json<Value>> {
    let state = state.service.store().read(&project).await?;
    Ok(envelope(
        "planning_proposals",
        state
            .get("planning_proposals")
            .cloned()
            .unwrap_or_else(|| super::json!([])),
    ))
}

pub(super) async fn decision(
    State(state): State<ApiState>,
    Path((project, plan)): Path<(String, String)>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> ApiResult<Json<Value>> {
    fields(&input, &["expected_revision", "decision", "reason"])?;
    Ok(envelope(
        "planning_proposal",
        state
            .service
            .planning_decision24(&project, &plan, &input, &idempotency(&headers)?)
            .await?,
    ))
}

pub(super) async fn mode(
    State(state): State<ApiState>,
    Path((project, run)): Path<(String, String)>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> ApiResult<Json<Value>> {
    fields(&input, &["expected_revision", "mode"])?;
    Ok(envelope(
        "run",
        state
            .service
            .set_mode24(&project, &run, &input, &idempotency(&headers)?)
            .await?,
    ))
}
