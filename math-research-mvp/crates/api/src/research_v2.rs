//! Version-isolated research transport. This router never mounts v1 handlers.
#![allow(clippy::too_many_lines, clippy::missing_errors_doc)]

use std::{convert::Infallible, sync::Arc, time::Duration};

use async_stream::stream;
use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Path, Query, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use research_core::research_v2::V2Service;
use research_domain::research_v2::{CONTRACT, VERSION, new_id, now};
use research_storage::research_v2::{V2Error, V2Result};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

mod collaboration23;
mod lifecycle;
mod whiteboard24;

const ACTOR: &str = "local-owner";
const MAX_TEXT_BYTES: usize = 1_000_000;
const ALLOWED_ORIGINS: &[&str] = &[
    "http://127.0.0.1:4334",
    "http://localhost:4334",
    "http://[::1]:4334",
    "http://127.0.0.1:8899",
    "http://localhost:8899",
    "http://[::1]:8899",
];

#[derive(Clone)]
struct ApiState {
    service: V2Service,
    token: Arc<String>,
    binding_lock: Arc<Mutex<()>>,
}

/// Requires a high-entropy secret supplied exclusively to the platform server.
///
/// # Panics
/// Panics if the supplied authentication secret contains fewer than 32 bytes.
pub fn router(service: V2Service, token: String) -> Router {
    assert!(
        token.len() >= 32,
        "v2 requires a nonempty high-entropy API token"
    );
    let state = ApiState {
        service,
        token: Arc::new(token),
        binding_lock: Arc::new(Mutex::new(())),
    };
    let routes = Router::new()
        .route("/health", get(health))
        .route("/version", get(health))
        .route("/capabilities", get(capabilities))
        .route("/projects", get(list_projects).post(create_project))
        .route("/projects/{p}", get(read_project).patch(patch_project))
        .route("/projects/{p}/snapshot", get(snapshot))
        .route(
            "/projects/{p}/problem-spec",
            get(whiteboard24::problem_spec).patch(whiteboard24::update_problem_spec),
        )
        .route("/projects/{p}/planning-proposals", get(whiteboard24::plans))
        .route(
            "/projects/{p}/planning-proposals/{plan}/decision",
            post(whiteboard24::decision),
        )
        .route("/projects/{p}/runs/{r}/mode", post(whiteboard24::mode))
        .route("/projects/{p}/proof-tree", get(collaboration23::proof_tree))
        .route("/projects/{p}/events", get(events))
        .route("/projects/{p}/runs", post(start_run))
        .route("/projects/{p}/runs/{r}", get(read_run))
        .route(
            "/projects/{p}/runs/{r}/limits",
            axum::routing::patch(lifecycle::update_limits),
        )
        .route(
            "/projects/{p}/feedback",
            get(collaboration23::feedback_traces).post(lifecycle::feedback),
        )
        .route(
            "/projects/{p}/feedback/{c}/withdraw",
            post(collaboration23::withdraw),
        )
        .route(
            "/projects/{p}/feedback/{c}/escalate",
            post(collaboration23::escalate),
        )
        .route(
            "/projects/{p}/human-questions",
            get(collaboration23::questions),
        )
        .route(
            "/projects/{p}/human-questions/{q}/answer",
            post(collaboration23::answer),
        )
        .route("/projects/{p}/cycles", get(lifecycle::cycles))
        .route("/projects/{p}/routes", get(lifecycle::routes))
        .route("/projects/{p}/messages", get(lifecycle::messages))
        .route("/projects/{p}/advisories", get(lifecycle::advisories))
        .route(
            "/projects/{p}/memory-entries",
            get(lifecycle::memory_entries),
        )
        .route(
            "/projects/{p}/memories/search",
            get(lifecycle::search_memories),
        )
        .route(
            "/projects/{p}/statements/{s}",
            get(lifecycle::read_statement),
        )
        .route(
            "/projects/{p}/display-summaries",
            get(lifecycle::display_summaries),
        )
        .route("/projects/{p}/jobs", get(lifecycle::background_jobs))
        .route(
            "/projects/{p}/proof-checkpoints",
            get(lifecycle::proof_checkpoints),
        )
        .route(
            "/projects/{p}/pending-assignments",
            get(lifecycle::pending_assignments),
        )
        .route("/projects/{p}/project-control", post(project_control))
        .route("/projects/{p}/model-selection", post(model_selection))
        .route("/projects/{p}/commands", post(command))
        .route("/projects/{p}/commands/{c}", get(read_command))
        .route("/projects/{p}/commands/{c}/cancel", post(cancel_command))
        .route("/projects/{p}/nodes", get(list_nodes))
        .route("/projects/{p}/nodes/{n}", get(read_node))
        .route("/projects/{p}/nodes/{n}/versions", get(node_versions))
        .route(
            "/projects/{p}/discussions",
            get(list_discussions).post(create_discussion),
        )
        .route("/projects/{p}/discussions/{d}", get(read_discussion))
        .route(
            "/projects/{p}/discussions/{d}/messages",
            post(discussion_message),
        )
        .route(
            "/projects/{p}/annotations",
            get(list_annotations).post(create_annotation),
        )
        .route(
            "/projects/{p}/annotations/{a}",
            axum::routing::patch(patch_annotation),
        )
        .route(
            "/projects/{p}/problem-revision-previews",
            post(create_preview),
        )
        .route(
            "/projects/{p}/problem-revision-previews/{v}",
            get(read_preview),
        )
        .route(
            "/projects/{p}/view-preferences",
            get(read_view).put(write_view),
        )
        .route(
            "/projects/{p}/conversation-bindings",
            post(bind_conversation),
        )
        .route("/projects/{p}/reports", get(list_reports))
        .route("/projects/{p}/artifacts/{a}/content", get(artifact_content))
        .route("/projects/{p}/material-imports", post(import_material))
        .route("/projects/{p}/evidence", get(evidence))
        .route("/projects/{p}/evidence/search", get(search_evidence))
        .route("/projects/{p}/memory/search", get(search_evidence))
        .route(
            "/projects/{p}/interaction-executions",
            get(list_interactions).post(create_interaction),
        )
        .route(
            "/projects/{p}/interaction-executions/{i}",
            get(read_interaction),
        )
        .route(
            "/projects/{p}/interaction-executions/{i}/cancel",
            post(cancel_interaction),
        )
        .fallback(not_found);
    Router::new()
        .route("/health", get(health))
        .route("/version", get(health))
        .nest("/api/v2/research", routes)
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024))
        .layer(middleware::from_fn_with_state(state.clone(), security))
        .with_state(state)
}

#[derive(Debug)]
struct ApiError(V2Error);
type ApiResult<T> = Result<T, ApiError>;

impl From<V2Error> for ApiError {
    fn from(value: V2Error) -> Self {
        Self(value)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let code = self.0.code.as_str();
        let status = match code {
            "AUTH_REQUIRED" => StatusCode::UNAUTHORIZED,
            "FORBIDDEN" | "WORKSPACE_DENIED" | "ORIGIN_DENIED" => StatusCode::FORBIDDEN,
            "NOT_FOUND" | "PROJECT_NOT_FOUND" | "ARTIFACT_NOT_FOUND" => StatusCode::NOT_FOUND,
            "REVISION_CONFLICT"
            | "IDEMPOTENCY_CONFLICT"
            | "ACTIVE_RUN_EXISTS"
            | "RUN_ACTIVE"
            | "RUN_ENDED"
            | "RUN_NOT_ACTIVE"
            | "PROJECT_PAUSED"
            | "PROJECT_STOPPED"
            | "CONTROL_PENDING"
            | "DELIVERY_UNCERTAIN"
            | "STALE_PROBLEM_VERSION"
            | "STALE_CONTROL_EPOCH"
            | "CURSOR_CONFLICT"
            | "INVALID_STATE" => StatusCode::CONFLICT,
            "PROVIDER_UNAVAILABLE" | "STORAGE_UNAVAILABLE" => StatusCode::SERVICE_UNAVAILABLE,
            _ => StatusCode::UNPROCESSABLE_ENTITY,
        };
        (status, Json(json!({"contract":CONTRACT,"error":{
            "code":self.0.code,"message":self.0.message,"retryable":status==StatusCode::SERVICE_UNAVAILABLE,
            "user_action_required":true,"safe_next_action":"检查请求与当前项目状态后再试。",
            "correlation_id":new_id(),"details":{"draft_preserved":false}
        }}))).into_response()
    }
}

fn invalid(message: impl Into<String>) -> ApiError {
    V2Error::new("INVALID_REQUEST", message.into()).into()
}

fn missing(message: &str) -> V2Error {
    V2Error::new("NOT_FOUND", message)
}

async fn security(State(state): State<ApiState>, request: Request, next: Next) -> Response {
    if let Some(origin) = request.headers().get(header::ORIGIN) {
        if !origin
            .to_str()
            .is_ok_and(|value| ALLOWED_ORIGINS.contains(&value))
        {
            return ApiError(V2Error::new(
                "ORIGIN_DENIED",
                "该网页来源未获准访问本地研究服务。",
            ))
            .into_response();
        }
    }
    if let Some(host) = request.headers().get(header::HOST) {
        let safe = host.to_str().is_ok_and(|value| {
            let name = value.split(':').next().unwrap_or("");
            name == "127.0.0.1"
                || name == "localhost"
                || value.starts_with("[::1]:")
                || value == "[::1]"
        });
        if !safe {
            return ApiError(V2Error::new("ORIGIN_DENIED", "只允许 loopback 主机名。"))
                .into_response();
        }
    }
    let public = matches!(
        request.uri().path(),
        "/health" | "/version" | "/api/v2/research/health" | "/api/v2/research/version"
    );
    if !public {
        let supplied = request
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "));
        if !supplied.is_some_and(|value| equal_secret(value.as_bytes(), state.token.as_bytes())) {
            return ApiError(V2Error::new(
                "AUTH_REQUIRED",
                "研究服务需要平台服务器认证；请勿把 API token 放进浏览器。",
            ))
            .into_response();
        }
    }
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; frame-ancestors 'none'; sandbox"),
    );
    response
}

fn equal_secret(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b)
        .fold(0_u8, |difference, (x, y)| difference | (x ^ y))
        == 0
}

async fn health() -> Json<Value> {
    Json(json!({"contract":CONTRACT,"version":VERSION,"service":"mathcat-v2","status":"ok"}))
}

async fn capabilities() -> Json<Value> {
    Json(
        json!({"contract":CONTRACT,"version":VERSION,"schema_version":1,
        "features":{"research_runs":true,"public_event_stream":true,"record_only_discussions":true,
        "annotations":true,"problem_revision_previews":true,"view_preferences":true,"artifact_search":true,
        "independent_interaction_execution":true,"read_only_explanations":true,"url_material_import":false,"formal_certification":false},
        "adapter_probe":"run_preflight_required","note":"执行器能力必须通过实际预检；此表仅声明传输层功能。"}),
    )
}

fn unsupported() -> ApiError {
    V2Error::new(
        "CAPABILITY_UNSUPPORTED",
        "此接口的执行能力尚未接入；没有创建付费任务或伪造结果。",
    )
    .into()
}

async fn not_found() -> ApiError {
    missing("v2 接口不存在；此服务不会转发到旧版研究内核。").into()
}

fn idempotency(headers: &HeaderMap) -> ApiResult<String> {
    let value = headers
        .get("Idempotency-Key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if value.is_empty()
        || value.len() > 200
        || !value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_.:".contains(&c))
    {
        return Err(invalid(
            "写操作必须提供 1–200 字节的 Idempotency-Key（字母、数字、-_.:）。",
        ));
    }
    Ok(value.to_owned())
}

fn fields(input: &Value, allowed: &[&str]) -> ApiResult<()> {
    let object = input
        .as_object()
        .ok_or_else(|| invalid("请求正文必须是 JSON 对象。"))?;
    for name in object.keys() {
        if !allowed.contains(&name.as_str()) {
            return Err(invalid(format!("不支持的请求字段：{name}")));
        }
    }
    Ok(())
}

fn required_text(input: &Value, name: &str) -> ApiResult<String> {
    let text = input.get(name).and_then(Value::as_str).unwrap_or("").trim();
    if text.is_empty() || text.len() > MAX_TEXT_BYTES {
        return Err(invalid(format!("{name} 必须是非空文本，且不超过 1 MB。")));
    }
    Ok(text.to_owned())
}

fn rows<'a>(project: &'a Value, field: &str) -> &'a [Value] {
    project
        .get(field)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn append(project: &mut Value, field: &str, row: Value) -> V2Result<()> {
    if project.get(field).is_none() {
        project[field] = json!([]);
    }
    project[field]
        .as_array_mut()
        .ok_or_else(|| V2Error::new("INVALID_STATE", "项目集合格式不正确。"))?
        .push(row);
    Ok(())
}

fn entity<'a>(project: &'a Value, collection: &str, id: &str) -> V2Result<&'a Value> {
    rows(project, collection)
        .iter()
        .find(|row| row["id"].as_str() == Some(id) || row["node_id"].as_str() == Some(id))
        .ok_or_else(|| missing("对象不存在或不属于此项目。"))
}

fn check_revision(object: &Value, expected: Option<u64>) -> V2Result<()> {
    let expected = expected
        .ok_or_else(|| V2Error::new("INVALID_REQUEST", "修改必须携带 expected_revision。"))?;
    if object["revision"].as_u64().unwrap_or(1) != expected {
        return Err(V2Error::new(
            "REVISION_CONFLICT",
            "对象已有新版本，请查看差异后再提交；请求未覆盖当前内容。",
        ));
    }
    Ok(())
}

fn fingerprint(input: &Value) -> String {
    format!("{:x}", Sha256::digest(input.to_string().as_bytes()))
}

async fn prior_receipt(
    state: &ApiState,
    project_id: &str,
    scope: &str,
    key: &str,
    input: &Value,
) -> ApiResult<Option<Value>> {
    let project = state.service.store().read(project_id).await?;
    let receipt_key = format!("{ACTOR}:{scope}:{key}");
    if let Some(old) = project
        .get("_api_receipts")
        .and_then(|v| v.get(&receipt_key))
    {
        if old["payload_hash"] != fingerprint(input) {
            return Err(
                V2Error::new("IDEMPOTENCY_CONFLICT", "同一幂等键不能用于不同请求。").into(),
            );
        }
        return Ok(Some(old["result"].clone()));
    }
    Ok(None)
}

async fn mutation<F>(
    state: &ApiState,
    project_id: &str,
    event: &str,
    scope: &str,
    key: &str,
    input: &Value,
    update: F,
) -> ApiResult<Value>
where
    F: FnOnce(&mut Value) -> V2Result<Value> + Send,
{
    let receipt_key = format!("{ACTOR}:{scope}:{key}");
    let payload_hash = fingerprint(input);
    let result = state
        .service
        .store()
        .mutate(project_id, event, None, move |project| {
            if let Some(old) = project
                .get("_api_receipts")
                .and_then(|v| v.get(&receipt_key))
            {
                if old["payload_hash"] != payload_hash {
                    return Err(V2Error::new(
                        "IDEMPOTENCY_CONFLICT",
                        "相同幂等键不能用于不同请求。",
                    ));
                }
                return Ok(old["result"].clone());
            }
            let result = update(project)?;
            if !project["_api_receipts"].is_object() {
                project["_api_receipts"] = json!({});
            }
            project["_api_receipts"][receipt_key] =
                json!({"payload_hash":payload_hash,"result":result});
            Ok(result)
        })
        .await?;
    Ok(result)
}

fn public_project(mut project: Value) -> Value {
    sanitize(&mut project);
    project
}

fn sanitize(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.retain(|key, _| {
                !key.starts_with('_')
                    && ![
                        "api_key",
                        "access_token",
                        "refresh_token",
                        "token",
                        "credentials",
                        "auth_profile_ref",
                        "native_session_ref",
                        "native_session_id",
                        "native_resume_ref",
                        "provider_request_ref",
                        "cancel_token",
                        "raw_jsonl",
                        "raw_stderr",
                        "process_id",
                        "pid",
                    ]
                    .contains(&key.as_str())
            });
            for nested in object.values_mut() {
                sanitize(nested);
            }
        }
        Value::Array(array) => {
            for nested in array {
                sanitize(nested);
            }
        }
        _ => {}
    }
}

fn envelope(field: &str, value: Value) -> Json<Value> {
    let mut response = json!({"contract":CONTRACT});
    response[field] = public_project(value);
    Json(response)
}

#[derive(Deserialize, Default)]
struct ListQuery {
    workspace_id: Option<String>,
    lifecycle: Option<String>,
    cursor: Option<String>,
    limit: Option<usize>,
}

fn page(values: &[Value], query: &ListQuery) -> ApiResult<Value> {
    let start = query.cursor.as_deref().map_or(Ok(0), |v| {
        v.strip_prefix("v2:")
            .ok_or_else(|| invalid("无效分页游标。"))?
            .parse::<usize>()
            .map_err(|_| invalid("无效分页游标。"))
    })?;
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    if start > values.len() {
        return Err(invalid("分页游标超出结果集。"));
    }
    let end = start.saturating_add(limit).min(values.len());
    Ok(
        json!({"items":values[start..end],"next_cursor":(end<values.len()).then(||format!("v2:{end}"))}),
    )
}

async fn create_project(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> ApiResult<impl IntoResponse> {
    let key = idempotency(&headers)?;
    fields(
        &input,
        &["title", "problem", "workspace_id", "material_refs"],
    )?;
    required_text(&input, "problem")?;
    if input
        .get("material_refs")
        .is_some_and(|v| v.as_array().is_none_or(|a| !a.is_empty()))
    {
        return Err(invalid(
            "先创建项目，再通过 material-imports 导入文字材料；不能引用未注册的跨项目文件。",
        ));
    }
    let project = state.service.store().create_project(input, &key).await?;
    Ok((
        StatusCode::CREATED,
        envelope("project", public_project(project)),
    ))
}

async fn list_projects(
    State(state): State<ApiState>,
    Query(query): Query<ListQuery>,
) -> ApiResult<Json<Value>> {
    let projects: Vec<Value> = state
        .service
        .store()
        .list_projects()
        .await?
        .into_iter()
        .filter(|project| {
            query
                .workspace_id
                .as_ref()
                .is_none_or(|id| project["workspace_id"].as_str() == Some(id))
                && query
                    .lifecycle
                    .as_ref()
                    .is_none_or(|v| project["lifecycle"].as_str() == Some(v))
        })
        .map(public_project)
        .collect();
    let paged = page(&projects, &query)?;
    Ok(Json(
        json!({"contract":CONTRACT,"projects":paged["items"],"next_cursor":paged["next_cursor"]}),
    ))
}

async fn read_project(
    State(state): State<ApiState>,
    Path(p): Path<String>,
) -> ApiResult<Json<Value>> {
    let mut project = state.service.store().read(&p).await?;
    project["model_selection"] = V2Service::model_selection_from_snapshot(&project);
    Ok(envelope("project", public_project(project)))
}

async fn snapshot(State(state): State<ApiState>, Path(p): Path<String>) -> ApiResult<Json<Value>> {
    let mut project = V2Service::whiteboard_from_snapshot24(&state.service.store().read(&p).await?);
    project["project_control"] = V2Service::project_control_from_snapshot(&project);
    project["model_selection"] = V2Service::model_selection_from_snapshot(&project);
    project["proof_tree"] = V2Service::proof_tree_from_snapshot23(&project);
    project["feedback_traces"] = json!(V2Service::feedback_traces_from_snapshot23(&project, None));
    if project["human_questions"].is_null() {
        project["human_questions"] = json!([]);
    }
    let project = public_project(project);
    let mut response = project.clone();
    response["contract"] = json!(CONTRACT);
    response["project"] = project;
    Ok(Json(response))
}

async fn model_selection(
    State(state): State<ApiState>,
    Path(p): Path<String>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> ApiResult<Json<Value>> {
    let key = idempotency(&headers)?;
    fields(&input, &["model", "reasoning_effort", "expected_revision"])?;
    let result = state.service.set_model_selection(&p, input, &key).await?;
    Ok(envelope("model_selection", result))
}

async fn project_control(
    State(state): State<ApiState>,
    Path(p): Path<String>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> ApiResult<impl IntoResponse> {
    let key = idempotency(&headers)?;
    fields(&input, &["type"])?;
    let result = state.service.project_control(&p, input, &key).await?;
    Ok((StatusCode::ACCEPTED, envelope("project_control", result)))
}

async fn patch_project(
    State(state): State<ApiState>,
    Path(p): Path<String>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> ApiResult<Json<Value>> {
    let key = idempotency(&headers)?;
    fields(&input, &["title", "lifecycle", "expected_revision"])?;
    if input.get("title").is_some() {
        required_text(&input, "title")?;
    }
    let value = input.clone();
    let result=mutation(&state,&p,"project.updated","project",&key,&input,move |project| {
        check_revision(project,value["expected_revision"].as_u64())?;
        if let Some(lifecycle)=value.get("lifecycle") {
            if !matches!(lifecycle.as_str(),Some("open"|"archived")) { return Err(V2Error::new("INVALID_REQUEST","lifecycle 不在合法枚举内。")); }
            if lifecycle=="archived" && rows(project,"runs").iter().any(|r|r["state"]!="ended") { return Err(V2Error::new("INVALID_STATE","归档前必须停止活动研究。")); }
            project["lifecycle"]=lifecycle.clone();
        }
        if let Some(title)=value.get("title") { project["title"]=title.clone(); }
        Ok(json!({"id":project["id"],"title":project["title"],"lifecycle":project["lifecycle"]}))
    }).await?;
    Ok(envelope("project", result))
}

async fn start_run(
    State(state): State<ApiState>,
    Path(p): Path<String>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> ApiResult<impl IntoResponse> {
    let key = idempotency(&headers)?;
    fields(
        &input,
        &[
            "mode",
            "duration_seconds",
            "limits",
            "adapter",
            "previous_run_id",
            "start_authorized",
            "wait_policy",
            "verification",
            "model",
            "reasoning_effort",
        ],
    )?;
    let result = state.service.start_run(&p, input, &key).await?;
    Ok((StatusCode::ACCEPTED, envelope("run", result)))
}

async fn read_run(
    State(state): State<ApiState>,
    Path((p, r)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    let project = state.service.store().read(&p).await?;
    Ok(envelope("run", entity(&project, "runs", &r)?.clone()))
}

async fn command(
    State(state): State<ApiState>,
    Path(p): Path<String>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> ApiResult<impl IntoResponse> {
    let key = idempotency(&headers)?;
    fields(
        &input,
        &[
            "contract",
            "type",
            "target",
            "run_id",
            "expected_versions",
            "apply_at",
            "payload",
            "priority",
            "urgency_reason",
        ],
    )?;
    if input.get("contract").is_some_and(|value| value != CONTRACT) {
        return Err(invalid("请求 contract 不匹配 v2。"));
    }
    let result = state.service.command(&p, input, &key).await?;
    Ok((StatusCode::ACCEPTED, envelope("command", result)))
}

async fn read_command(
    State(state): State<ApiState>,
    Path((p, c)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    let project = state.service.store().read(&p).await?;
    Ok(envelope(
        "command",
        entity(&project, "commands", &c)?.clone(),
    ))
}

async fn cancel_command(
    State(state): State<ApiState>,
    Path((p, c)): Path<(String, String)>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> ApiResult<Json<Value>> {
    let key = idempotency(&headers)?;
    fields(&input, &["expected_revision"])?;
    let project = state.service.store().read(&p).await?;
    if entity(&project, "commands", &c)?
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|kind| {
            matches!(
                kind,
                "suggest_idea" | "steer_focus" | "restrict_method" | "feedback_correction"
            )
        })
    {
        let command = state
            .service
            .change_feedback23(&p, &c, "withdraw", &input, &key)
            .await?;
        return Ok(envelope("command", command));
    }
    let scope = format!("cancel-command:{c}");
    let expected = input["expected_revision"].as_u64();
    let result = mutation(
        &state,
        &p,
        "command.cancelled",
        &scope,
        &key,
        &input,
        move |project| {
            let existing = entity(project, "commands", &c)?.clone();
            check_revision(&existing, expected)?;
            if !matches!(existing["status"].as_str(), Some("accepted" | "queued")) {
                return Err(V2Error::new(
                    "INVALID_STATE",
                    "命令可能已经交付；不能撤销已发生的研究，请提交新指令。",
                ));
            }
            let target = project["commands"]
                .as_array_mut()
                .and_then(|r| r.iter_mut().find(|r| r["id"] == c))
                .ok_or_else(|| missing("命令不存在。"))?;
            target["status"] = json!("cancelled");
            target["revision"] = json!(existing["revision"].as_u64().unwrap_or(1) + 1);
            target["cancelled_at"] = json!(now());
            Ok(target.clone())
        },
    )
    .await?;
    Ok(envelope("command", result))
}

#[derive(Deserialize, Default)]
struct EventQuery {
    after: Option<u64>,
    once: Option<bool>,
    format: Option<String>,
}

async fn events(
    State(state): State<ApiState>,
    Path(p): Path<String>,
    headers: HeaderMap,
    Query(query): Query<EventQuery>,
) -> ApiResult<Response> {
    state.service.store().read(&p).await?;
    let header_cursor = headers
        .get("Last-Event-ID")
        .map(|v| {
            v.to_str()
                .map_err(|_| invalid("无效事件游标。"))?
                .parse::<u64>()
                .map_err(|_| invalid("无效事件游标。"))
        })
        .transpose()?;
    if query.after.zip(header_cursor).is_some_and(|(a, b)| a != b) {
        return Err(V2Error::new("CURSOR_CONFLICT", "after 与 Last-Event-ID 不一致。").into());
    }
    let mut cursor = query.after.or(header_cursor).unwrap_or(0);
    let first = state.service.store().events(&p, cursor).await?;
    if query.format.as_deref() == Some("json") {
        let next_cursor = first
            .last()
            .and_then(|e| e["seq"].as_u64())
            .unwrap_or(cursor);
        return Ok(Json(json!({"contract":CONTRACT,"events":first.into_iter().map(public_project).collect::<Vec<_>>(),"event_cursor":next_cursor})).into_response());
    }
    let output = stream! {
        let mut batch=first;
        loop {
            for record in batch {
                let sequence=record["seq"].as_u64().unwrap_or(cursor);
                if sequence<=cursor {continue;}
                cursor=sequence;
                let public=public_project(record);
                let message=Event::default().id(sequence.to_string()).event(public["type"].as_str().unwrap_or("research.event")).data(public.to_string());
                yield Ok::<Event,Infallible>(message);
            }
            if query.once==Some(true){break;}
            tokio::time::sleep(Duration::from_millis(500)).await;
            match state.service.store().events(&p,cursor).await {
                Ok(next)=>batch=next,
                Err(error)=>{yield Ok(Event::default().event("stream.error").data(json!({"contract":CONTRACT,"error":{"code":error.code,"message":error.message}}).to_string()));break;}
            }
        }
    };
    Ok(Sse::new(output)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("mathcat-v2"),
        )
        .into_response())
}

async fn list_nodes(
    State(state): State<ApiState>,
    Path(p): Path<String>,
) -> ApiResult<Json<Value>> {
    let project = state.service.store().read(&p).await?;
    Ok(envelope("nodes", json!(rows(&project, "nodes"))))
}

#[derive(Deserialize, Default)]
struct NodeQuery {
    revision: Option<u64>,
}

fn exact_node(project: &Value, id: &str, revision: Option<u64>) -> V2Result<Value> {
    let node = entity(project, "nodes", id)?;
    if revision.is_none_or(|r| node["revision"].as_u64().unwrap_or(1) == r) {
        return Ok(node.clone());
    }
    rows(project, "node_versions")
        .iter()
        .chain(rows(node, "versions"))
        .find(|v| (v["node_id"] == id || v["id"] == id) && v["revision"].as_u64() == revision)
        .cloned()
        .ok_or_else(|| missing("指定节点版本不存在。"))
}

async fn read_node(
    State(state): State<ApiState>,
    Path((p, n)): Path<(String, String)>,
    Query(query): Query<NodeQuery>,
) -> ApiResult<Json<Value>> {
    let project = state.service.store().read(&p).await?;
    Ok(envelope("node", exact_node(&project, &n, query.revision)?))
}

async fn node_versions(
    State(state): State<ApiState>,
    Path((p, n)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    let project = state.service.store().read(&p).await?;
    let node = entity(&project, "nodes", &n)?;
    let mut versions: Vec<Value> = rows(&project, "node_versions")
        .iter()
        .filter(|v| v["node_id"] == n)
        .cloned()
        .collect();
    versions.extend(rows(node, "versions").iter().cloned());
    if !versions.iter().any(|v| v["revision"] == node["revision"]) {
        versions.push(node.clone());
    }
    Ok(envelope("versions", json!(versions)))
}

async fn list_discussions(
    State(state): State<ApiState>,
    Path(p): Path<String>,
) -> ApiResult<Json<Value>> {
    let project = state.service.store().read(&p).await?;
    Ok(envelope(
        "discussions",
        json!(rows(&project, "discussions")),
    ))
}

async fn read_discussion(
    State(state): State<ApiState>,
    Path((p, d)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    let project = state.service.store().read(&p).await?;
    Ok(envelope(
        "discussion",
        entity(&project, "discussions", &d)?.clone(),
    ))
}

fn validate_node_ref(project: &Value, reference: &Value) -> V2Result<()> {
    let id = reference["node_id"]
        .as_str()
        .ok_or_else(|| V2Error::new("INVALID_REQUEST", "node_version_ref 缺少 node_id。"))?;
    let revision = reference["revision"]
        .as_u64()
        .ok_or_else(|| V2Error::new("INVALID_REQUEST", "node_version_ref 缺少 revision。"))?;
    exact_node(project, id, Some(revision))?;
    Ok(())
}

async fn create_discussion(
    State(state): State<ApiState>,
    Path(p): Path<String>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> ApiResult<impl IntoResponse> {
    let key = idempotency(&headers)?;
    fields(
        &input,
        &[
            "node_version_ref",
            "selection_ref",
            "initial_message",
            "reply_mode",
        ],
    )?;
    if input.get("reply_mode").is_some_and(|v| v != "record_only") {
        return Err(unsupported());
    }
    let value = input.clone();
    let result=mutation(&state,&p,"discussion.created","discussion",&key,&input,move|project|{
        if let Some(reference)=value.get("node_version_ref").filter(|v|!v.is_null()){validate_node_ref(project,reference)?;}
        let selection=research_core::research_v2::interactions::selection23(project,&value["selection_ref"])?;
        let messages=value["initial_message"].as_str().filter(|s|!s.trim().is_empty()).map_or_else(Vec::new,|text|vec![json!({"id":new_id(),"author":ACTOR,"text":text,"created_at":now(),"reply_mode":"record_only"})]);
        let row=json!({"id":new_id(),"selection":selection,"node_version_ref":value["node_version_ref"],"messages":messages,"linked_command_ids":[],"state":"open","revision":1,"created_by":ACTOR,"created_at":now()});
        append(project,"discussions",row.clone())?;Ok(row)
    }).await?;
    Ok((StatusCode::CREATED, envelope("discussion", result)))
}

async fn discussion_message(
    State(state): State<ApiState>,
    Path((p, d)): Path<(String, String)>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> ApiResult<impl IntoResponse> {
    let key = idempotency(&headers)?;
    fields(
        &input,
        &[
            "text",
            "attachment_refs",
            "reply_mode",
            "execution_owner",
            "selection_ref",
        ],
    )?;
    let text = required_text(&input, "text")?;
    if input["reply_mode"] == "explain" {
        let result = state
            .service
            .explain_discussion(&p, &d, input, &key)
            .await?;
        return Ok((StatusCode::ACCEPTED, envelope("execution", result)));
    }
    if input.get("reply_mode").is_some_and(|v| v != "record_only") {
        return Err(unsupported());
    }
    if input.get("execution_owner").is_some() {
        return Err(invalid("record_only 不得创建执行归属。"));
    }
    let scope = format!("discussion:{d}");
    let attachments = input.get("attachment_refs").cloned().unwrap_or(json!([]));
    let selection_ref = input["selection_ref"].clone();
    let result=mutation(&state,&p,"discussion.message_created",&scope,&key,&input,move|project|{
        for attachment in attachments.as_array().ok_or_else(||V2Error::new("INVALID_REQUEST","attachment_refs 必须是数组。"))?{
            let id=attachment.as_str().or_else(||attachment["artifact_id"].as_str()).ok_or_else(||V2Error::new("INVALID_REQUEST","附件必须使用 artifact id。"))?;
            entity(project,"artifacts",id)?;
        }
        let selection=research_core::research_v2::interactions::selection23(project,&selection_ref)?;
        let target=project["discussions"].as_array_mut().and_then(|r|r.iter_mut().find(|r|r["id"]==d)).ok_or_else(||missing("讨论不存在。"))?;
        let row=json!({"id":new_id(),"author":ACTOR,"text":text,"attachment_refs":attachments,"selection":selection,"reply_mode":"record_only","created_at":now()});
        append(target,"messages",row.clone())?;target["revision"]=json!(target["revision"].as_u64().unwrap_or(1)+1);Ok(row)
    }).await?;
    Ok((StatusCode::CREATED, envelope("message", result)))
}

async fn list_annotations(
    State(state): State<ApiState>,
    Path(p): Path<String>,
) -> ApiResult<Json<Value>> {
    let project = state.service.store().read(&p).await?;
    Ok(envelope(
        "annotations",
        json!(rows(&project, "annotations")),
    ))
}

async fn create_interaction(
    State(state): State<ApiState>,
    Path(p): Path<String>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> ApiResult<impl IntoResponse> {
    let key = idempotency(&headers)?;
    fields(
        &input,
        &[
            "discussion_id",
            "limits",
            "explicit_authorization",
            "expected_run_state",
        ],
    )?;
    let row = state.service.create_interaction(&p, input, &key).await?;
    Ok((StatusCode::CREATED, envelope("interaction", row)))
}

async fn read_interaction(
    State(state): State<ApiState>,
    Path((p, i)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    let project = state.service.store().read(&p).await?;
    Ok(envelope(
        "interaction",
        entity(&project, "interactions", &i)?.clone(),
    ))
}

async fn list_interactions(
    State(state): State<ApiState>,
    Path(p): Path<String>,
) -> ApiResult<Json<Value>> {
    let project = state.service.store().read(&p).await?;
    Ok(envelope(
        "interactions",
        json!(rows(&project, "interactions")),
    ))
}

async fn cancel_interaction(
    State(state): State<ApiState>,
    Path((p, i)): Path<(String, String)>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> ApiResult<Json<Value>> {
    let key = idempotency(&headers)?;
    fields(&input, &["expected_revision"])?;
    Ok(envelope(
        "interaction",
        state
            .service
            .cancel_interaction(&p, &i, input, &key)
            .await?,
    ))
}

async fn create_annotation(
    State(state): State<ApiState>,
    Path(p): Path<String>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> ApiResult<impl IntoResponse> {
    let key = idempotency(&headers)?;
    fields(
        &input,
        &["node_id", "anchor", "selection_ref", "kind", "body"],
    )?;
    if input["selection_ref"].is_null() {
        required_text(&input, "node_id")?;
    }
    required_text(&input, "body")?;
    let kind = input["kind"].as_str().unwrap_or("comment");
    if ![
        "comment",
        "question",
        "human_endorsement",
        "human_contribution",
        "summary_error",
    ]
    .contains(&kind)
    {
        return Err(invalid(
            "无效批注类型；正式质疑必须使用 challenge_evidence 命令。",
        ));
    }
    let value = input.clone();
    let result=mutation(&state,&p,"annotation.created","annotation",&key,&input,move|project|{
        let selection=if value["selection_ref"].is_null(){
            let identifier=value["node_id"].as_str().unwrap_or_default();
            let node=exact_node(project,identifier,value["anchor"]["node_revision"].as_u64())?;
            research_core::research_v2::interactions::selection23(project,&json!({"kind":"node","id":identifier,"revision":node["revision"],"artifact_id":value["anchor"]["artifact_id"]}))?
        }else{research_core::research_v2::interactions::selection23(project,&value["selection_ref"])?};
        if let Some(node_id)=value["node_id"].as_str(){if selection["kind"]!="node"||selection["id"]!=node_id{return Err(V2Error::new("REVISION_CONFLICT","批注节点与所选原文不一致"));}}
        let mut anchor=value.get("anchor").cloned().unwrap_or(json!({}));
        if !anchor.is_object(){return Err(V2Error::new("INVALID_REQUEST","anchor 必须是对象。"));}
        anchor["node_revision"]=if selection["kind"]=="node"{selection["revision"].clone()}else{Value::Null};anchor["artifact_id"]=selection["artifact_id"].clone();
        let row=json!({"id":new_id(),"node_id":if selection["kind"]=="node"{selection["id"].clone()}else{Value::Null},"selection_ref":selection,"anchor":anchor,"author":ACTOR,"body":value["body"],"kind":value.get("kind").unwrap_or(&json!("comment")),"resolution":"open","revision":1,"versions":[],"created_at":now()});
        append(project,"annotations",row.clone())?;Ok(row)
    }).await?;
    Ok((StatusCode::CREATED, envelope("annotation", result)))
}

async fn patch_annotation(
    State(state): State<ApiState>,
    Path((p, a)): Path<(String, String)>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> ApiResult<Json<Value>> {
    let key = idempotency(&headers)?;
    fields(
        &input,
        &["body", "resolution", "response_refs", "expected_revision"],
    )?;
    if input.get("body").is_some() {
        required_text(&input, "body")?;
    }
    if let Some(resolution) = input.get("resolution") {
        if !matches!(
            resolution.as_str(),
            Some("open" | "addressed" | "dismissed" | "superseded")
        ) {
            return Err(invalid("无效批注解决状态。"));
        }
        if resolution == "addressed" && input["response_refs"].as_array().is_none_or(Vec::is_empty)
        {
            return Err(invalid("addressed 必须提供 response_refs。"));
        }
    }
    let value = input.clone();
    let scope = format!("annotation:{a}");
    let result = mutation(
        &state,
        &p,
        "annotation.updated",
        &scope,
        &key,
        &input,
        move |project| {
            let target = project["annotations"]
                .as_array_mut()
                .and_then(|r| r.iter_mut().find(|r| r["id"] == a))
                .ok_or_else(|| missing("批注不存在。"))?;
            if target["author"] != ACTOR {
                return Err(V2Error::new("FORBIDDEN", "不能修改他人的原始批注。"));
            }
            check_revision(target, value["expected_revision"].as_u64())?;
            let mut old = target.clone();
            if let Some(object) = old.as_object_mut() {
                object.remove("versions");
            }
            append(target, "versions", old)?;
            for name in ["body", "resolution", "response_refs"] {
                if let Some(v) = value.get(name) {
                    target[name] = v.clone();
                }
            }
            target["revision"] = json!(target["revision"].as_u64().unwrap_or(1) + 1);
            target["updated_at"] = json!(now());
            Ok(target.clone())
        },
    )
    .await?;
    Ok(envelope("annotation", result))
}

async fn create_preview(
    State(state): State<ApiState>,
    Path(p): Path<String>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> ApiResult<impl IntoResponse> {
    let key = idempotency(&headers)?;
    let _guard = state.binding_lock.lock().await;
    if let Some(prior) = prior_receipt(&state, &p, "problem-preview", &key, &input).await? {
        return Ok((StatusCode::CREATED, envelope("preview", prior)));
    }
    fields(
        &input,
        &[
            "new_problem",
            "problem",
            "new_statement_artifact_id",
            "new_assumptions_artifact_id",
            "goal_spec",
            "change_reason",
            "base_problem_version",
            "run_id",
            "control_epoch",
        ],
    )?;
    let mut value = input.clone();
    if !value["new_assumptions_artifact_id"].is_null() || !value["goal_spec"].is_null() {
        return Err(invalid(
            "首版请将全部假设和目标写入完整 new_problem；独立假设/目标字段暂不支持。",
        ));
    }
    if let Some(artifact_id) = value["new_statement_artifact_id"].as_str() {
        let (_, bytes) = state.service.store().read_artifact(&p, artifact_id).await?;
        let statement =
            String::from_utf8(bytes).map_err(|_| invalid("问题材料必须为 UTF-8 文本。"))?;
        if statement.trim().is_empty() || statement.len() > MAX_TEXT_BYTES {
            return Err(invalid("问题材料为空或过大。"));
        }
        if value
            .get("new_problem")
            .or_else(|| value.get("problem"))
            .and_then(Value::as_str)
            .is_some_and(|text| text != statement)
        {
            return Err(invalid("问题正文与指定材料不一致。"));
        }
        value["new_problem"] = json!(statement);
    }
    if !value["new_statement_artifact_id"].is_string() {
        let text = value["new_problem"]
            .as_str()
            .or_else(|| value["problem"].as_str())
            .unwrap_or("");
        if text.trim().is_empty() || text.len() > MAX_TEXT_BYTES {
            return Err(invalid(
                "必须提供完整 new_problem 或已导入的 statement artifact。",
            ));
        }
        let artifact = state
            .service
            .store()
            .put_artifact(&p, "proposed-problem.md", text.as_bytes(), "text/markdown")
            .await?;
        value["new_statement_artifact_id"] = artifact["id"].clone();
    }
    let result=mutation(&state,&p,"problem.preview_created","problem-preview",&key,&input,move|project|{
        let version=project["problem_version"].as_u64().unwrap_or(1);
        if value["base_problem_version"].as_u64()!=Some(version){return Err(V2Error::new("STALE_PROBLEM_VERSION","改题预览必须基于当前 problem_version。"));}
        let artifact_id=value["new_statement_artifact_id"].as_str().unwrap_or("");entity(project,"artifacts",artifact_id)?;
        if let Some(assumptions)=value["new_assumptions_artifact_id"].as_str(){entity(project,"artifacts",assumptions)?;}
        let run=rows(project,"runs").iter().find(|r|r["state"]!="ended").cloned().unwrap_or(Value::Null);
        if value.get("run_id").is_some()&&value["run_id"]!=run["id"]{return Err(V2Error::new("REVISION_CONFLICT","活动 Run 已改变。"));}
        if value.get("control_epoch").is_some()&&value["control_epoch"]!=run["control_epoch"]{return Err(V2Error::new("STALE_CONTROL_EPOCH","研究执行代际已改变。"));}
        let proposal=json!({"new_statement_artifact_id":value["new_statement_artifact_id"],"new_assumptions_artifact_id":value["new_assumptions_artifact_id"],"goal_spec":value["goal_spec"],"change_reason":value["change_reason"]});
        let affected:Vec<Value>=["tasks","candidates","reviews","facts"].iter().flat_map(|name|rows(project,name).iter().map(move|r|json!({"kind":name,"id":r["id"],"revision":r["revision"],"state":r["state"],"status":r["status"]}))).collect();
        let row=json!({"id":new_id(),"actor_id":ACTOR,"base_problem_version":version,"base_run_id":run["id"],"base_control_epoch":run["control_epoch"],"proposal":proposal,"proposal_payload_hash":fingerprint(&proposal),"impact_set_hash":fingerprint(&json!(affected)),"affected_refs":affected,"new_problem":value.get("new_problem").or_else(||value.get("problem")),"warnings":["改题不保证数学等价；旧执行结果隔离，不延长原截止时间。"],"created_at":now(),"expires_at":(Utc::now()+chrono::Duration::minutes(10)).to_rfc3339()});
        append(project,"previews",row.clone())?;Ok(row)
    }).await?;
    Ok((StatusCode::CREATED, envelope("preview", result)))
}

async fn read_preview(
    State(state): State<ApiState>,
    Path((p, v)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    let project = state.service.store().read(&p).await?;
    let mut value = entity(&project, "previews", &v)?.clone();
    value["expired"] = json!(
        value["expires_at"]
            .as_str()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .is_none_or(|date| date < Utc::now())
    );
    Ok(envelope("preview", value))
}

async fn read_view(State(state): State<ApiState>, Path(p): Path<String>) -> ApiResult<Json<Value>> {
    Ok(envelope(
        "view_preferences",
        state
            .service
            .store()
            .get_view_preferences(&p, ACTOR)
            .await?,
    ))
}

async fn write_view(
    State(state): State<ApiState>,
    Path(p): Path<String>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> ApiResult<Json<Value>> {
    let key = idempotency(&headers)?;
    fields(
        &input,
        &[
            "layout_version",
            "positions",
            "filters",
            "collapsed_ids",
            "selected_view",
            "expected_revision",
        ],
    )?;
    Ok(envelope(
        "view_preferences",
        state
            .service
            .store()
            .set_view_preferences(&p, ACTOR, input, &key)
            .await?,
    ))
}

async fn bind_conversation(
    State(state): State<ApiState>,
    Path(p): Path<String>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> ApiResult<impl IntoResponse> {
    let key = idempotency(&headers)?;
    fields(&input, &["platform_conversation_id", "kind", "node_id"])?;
    let conversation = required_text(&input, "platform_conversation_id")?;
    if !matches!(input["kind"].as_str(), Some("main" | "discussion")) {
        return Err(invalid("kind 必须是 main 或 discussion。"));
    }
    let _guard = state.binding_lock.lock().await;
    for other in state.service.store().list_projects().await? {
        if other["id"] != p
            && rows(&other, "conversation_bindings")
                .iter()
                .any(|b| b["platform_conversation_id"] == conversation)
        {
            return Err(V2Error::new("REVISION_CONFLICT", "这个对话已绑定到其他研究项目。").into());
        }
    }
    let value = input.clone();
    let result=mutation(&state,&p,"conversation.bound","conversation-binding",&key,&input,move|project|{
        if let Some(node)=value["node_id"].as_str(){entity(project,"nodes",node)?;}
        if let Some(existing)=rows(project,"conversation_bindings").iter().find(|b|b["platform_conversation_id"]==conversation){
            if existing["kind"]!=value["kind"]||existing["node_id"]!=value["node_id"]{return Err(V2Error::new("REVISION_CONFLICT","对话已有不同关联，不能静默替换。"));}return Ok(existing.clone());
        }
        let row=json!({"id":new_id(),"platform_conversation_id":conversation,"kind":value["kind"],"node_id":value["node_id"],"created_by":ACTOR,"created_at":now()});append(project,"conversation_bindings",row.clone())?;Ok(row)
    }).await?;
    Ok((StatusCode::CREATED, envelope("binding", result)))
}

#[derive(Deserialize, Default)]
struct ReportQuery {
    run_id: Option<String>,
}

async fn list_reports(
    State(state): State<ApiState>,
    Path(p): Path<String>,
    Query(query): Query<ReportQuery>,
) -> ApiResult<Json<Value>> {
    let project = state.service.store().read(&p).await?;
    let reports: Vec<Value> = rows(&project, "reports")
        .iter()
        .filter(|r| {
            query
                .run_id
                .as_ref()
                .is_none_or(|id| r["run_id"].as_str() == Some(id))
        })
        .cloned()
        .collect();
    Ok(envelope("reports", json!(reports)))
}

async fn artifact_content(
    State(state): State<ApiState>,
    Path((p, a)): Path<(String, String)>,
) -> ApiResult<Response> {
    let (metadata, content) = state.service.store().read_artifact(&p, &a).await?;
    let declared = metadata["media_type"]
        .as_str()
        .unwrap_or("application/octet-stream");
    let safe = match declared {
        "text/markdown" | "text/plain" | "application/json" | "application/pdf" => declared,
        _ => "application/octet-stream",
    };
    let mut response = Response::new(Body::from(content));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(safe).unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment"),
    );
    Ok(response)
}

async fn import_material(
    State(state): State<ApiState>,
    Path(p): Path<String>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> ApiResult<impl IntoResponse> {
    let key = idempotency(&headers)?;
    let _guard = state.binding_lock.lock().await;
    if let Some(prior) = prior_receipt(&state, &p, "material-import", &key, &input).await? {
        return Ok((StatusCode::CREATED, envelope("artifact", prior)));
    }
    fields(
        &input,
        &[
            "filename",
            "content",
            "media_type",
            "provenance",
            "material_role",
        ],
    )?;
    let material_role = input["material_role"]
        .as_str()
        .unwrap_or("unclassified_input");
    if !matches!(
        material_role,
        "problem_statement" | "reference" | "unclassified_input"
    ) || input
        .get("material_role")
        .is_some_and(|role| !role.is_string())
    {
        return Err(invalid(
            "material_role 必须为 problem_statement/reference/unclassified_input。",
        ));
    }
    let material_role = material_role.to_owned();
    if let Some(provenance) = input.get("provenance") {
        fields(
            provenance,
            &["original_filename", "extraction_method", "source_sha256"],
        )?;
    }
    let filename = required_text(&input, "filename")?;
    // Material is evidence: validate whitespace without altering the supplied UTF-8 bytes.
    let content = input["content"].as_str().unwrap_or("");
    if content.trim().is_empty() || content.len() > MAX_TEXT_BYTES {
        return Err(invalid("content 必须是非空文本，且不超过 1 MB。"));
    }
    if filename.contains(['/', '\\'])
        || filename.contains("..")
        || filename.to_lowercase().contains("credential")
        || filename.to_lowercase().contains("auth.json")
    {
        return Err(invalid("只接受非敏感文字材料的普通文件名。"));
    }
    let media = input["media_type"].as_str().unwrap_or("text/plain");
    if !matches!(
        media,
        "text/plain" | "text/markdown" | "text/x-tex" | "application/json"
    ) {
        return Err(invalid(
            "当前导入接口仅支持明确提交的文字材料；本地路径/URL/二进制导入尚未接入。",
        ));
    }
    let artifact = state
        .service
        .store()
        .put_artifact(&p, &filename, content.as_bytes(), media)
        .await?;
    let artifact_id = artifact["id"].clone();
    let provenance = input["provenance"].clone();
    let result = mutation(
        &state,
        &p,
        "material.imported",
        "material-import",
        &key,
        &input,
        move |project| {
            let problem_version = project["problem_version"].clone();
            let target = project["artifacts"]
                .as_array_mut()
                .and_then(|items| items.iter_mut().find(|item| item["id"] == artifact_id))
                .ok_or_else(|| missing("材料未注册。"))?;
            target["source_type"] = json!("user_import");
            target["problem_version"] = problem_version;
            target["material_role"] = json!(material_role);
            target["imported_by"] = json!(ACTOR);
            target["provenance"] = provenance;
            Ok(target.clone())
        },
    )
    .await?;
    Ok((StatusCode::CREATED, envelope("artifact", result)))
}

async fn evidence(State(state): State<ApiState>, Path(p): Path<String>) -> ApiResult<Json<Value>> {
    let project = state.service.store().read(&p).await?;
    Ok(Json(public_project(
        json!({"contract":CONTRACT,"candidates":rows(&project,"candidates"),"facts":rows(&project,"facts"),"reviews":rows(&project,"reviews"),"artifacts":rows(&project,"artifacts"),"memories":rows(&project,"memories")}),
    )))
}

#[derive(Deserialize, Default)]
struct SearchQuery {
    q: Option<String>,
    query: Option<String>,
}

async fn search_evidence(
    State(state): State<ApiState>,
    Path(p): Path<String>,
    Query(query): Query<SearchQuery>,
) -> ApiResult<Json<Value>> {
    let term = query.q.or(query.query).unwrap_or_default();
    if term.trim().is_empty() || term.len() > 4096 {
        return Err(invalid("搜索词必须非空且不超过 4096 字节。"));
    }
    let project = state.service.store().read(&p).await?;
    let needle = term.to_lowercase();
    let mut results = state.service.store().memory_search(&p, &term).await?;
    results.extend(
        rows(&project, "facts")
            .iter()
            .filter(|item| item.to_string().to_lowercase().contains(&needle))
            .map(|item| json!({"kind":"facts","record":item}))
            .take(40),
    );
    results.truncate(200);
    Ok(envelope("results", json!(results)))
}

#[cfg(test)]
mod tests;
