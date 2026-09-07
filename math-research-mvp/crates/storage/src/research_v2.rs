//! Independent v2 aggregate store, immutable evidence and transactional event journal.
//! No v1 table or migration is opened by this module.
#![allow(clippy::too_many_lines)]
use research_domain::research_v2::{CONTRACT, RunState, VERSION, deadline_reached, new_id, now};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{
    Row, SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use std::{
    path::{Component, Path, PathBuf},
    time::Duration,
};
use tokio::io::AsyncWriteExt;

#[derive(Debug, thiserror::Error)]
#[error("{code}: {message}")]
pub struct V2Error {
    pub code: String,
    pub message: String,
}
impl V2Error {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}
impl From<sqlx::Error> for V2Error {
    fn from(value: sqlx::Error) -> Self {
        Self::new("STORAGE_ERROR", value.to_string())
    }
}
impl From<std::io::Error> for V2Error {
    fn from(value: std::io::Error) -> Self {
        Self::new("FILE_ERROR", value.to_string())
    }
}
impl From<serde_json::Error> for V2Error {
    fn from(value: serde_json::Error) -> Self {
        Self::new("INVALID_JSON", value.to_string())
    }
}
pub type V2Result<T> = Result<T, V2Error>;

#[derive(Clone, Debug)]
pub struct V2Store {
    pool: SqlitePool,
    root: PathBuf,
}

fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
fn text<'a>(value: &'a Value, field: &str) -> &'a str {
    value[field].as_str().unwrap_or("")
}
fn array(value: &Value, field: &str) -> Vec<Value> {
    value[field].as_array().cloned().unwrap_or_default()
}

impl V2Store {
    pub async fn connect(db_path: &Path, data_root: &Path) -> V2Result<Self> {
        if let Some(parent) = db_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::create_dir_all(data_root).await?;
        let root = tokio::fs::canonicalize(data_root).await?;
        let options = SqliteConnectOptions::new()
            .filename(db_path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(10));
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        for statement in [
            "CREATE TABLE IF NOT EXISTS v2_schema(version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL)",
            "CREATE TABLE IF NOT EXISTS v2_projects(id TEXT PRIMARY KEY, document TEXT NOT NULL, revision INTEGER NOT NULL, cursor INTEGER NOT NULL)",
            "CREATE TABLE IF NOT EXISTS v2_events(project_id TEXT NOT NULL REFERENCES v2_projects(id), seq INTEGER NOT NULL, document TEXT NOT NULL, PRIMARY KEY(project_id,seq))",
            "CREATE TABLE IF NOT EXISTS v2_create_keys(key TEXT PRIMARY KEY, request_hash TEXT NOT NULL, project_id TEXT NOT NULL REFERENCES v2_projects(id))",
            "CREATE TABLE IF NOT EXISTS v2_views(project_id TEXT NOT NULL REFERENCES v2_projects(id), actor TEXT NOT NULL, document TEXT NOT NULL, PRIMARY KEY(project_id,actor))",
            "CREATE TABLE IF NOT EXISTS v2_view_keys(project_id TEXT NOT NULL, actor TEXT NOT NULL, key TEXT NOT NULL, request_hash TEXT NOT NULL, response TEXT NOT NULL, PRIMARY KEY(project_id,actor,key))",
        ] {
            sqlx::query(statement).execute(&pool).await?;
        }
        sqlx::query("INSERT OR IGNORE INTO v2_schema(version,applied_at) VALUES(1,?)")
            .bind(now())
            .execute(&pool)
            .await?;
        Ok(Self { pool, root })
    }

    #[must_use]
    pub fn data_root(&self) -> &Path {
        &self.root
    }

    pub async fn create_project(&self, input: Value, key: &str) -> V2Result<Value> {
        if key.is_empty() || key.len() > 200 {
            return Err(V2Error::new(
                "IDEMPOTENCY_REQUIRED",
                "An idempotency key is required.",
            ));
        }
        let problem = input["problem"]
            .as_str()
            .ok_or_else(|| V2Error::new("INVALID_INPUT", "problem must be text"))?;
        if problem.trim().is_empty() || problem.len() > 2_000_000 {
            return Err(V2Error::new(
                "INVALID_INPUT",
                "Problem is empty or exceeds the 2 MB material limit.",
            ));
        }
        let hash = digest(&serde_json::to_vec(&input)?);
        let mut tx = self.pool.begin().await?;
        if let Some(row) =
            sqlx::query("SELECT request_hash,project_id FROM v2_create_keys WHERE key=?")
                .bind(key)
                .fetch_optional(&mut *tx)
                .await?
        {
            if row.get::<String, _>("request_hash") != hash {
                return Err(V2Error::new(
                    "IDEMPOTENCY_CONFLICT",
                    "The key belongs to a different request.",
                ));
            }
            let id: String = row.get("project_id");
            let document: String =
                sqlx::query_scalar("SELECT document FROM v2_projects WHERE id=?")
                    .bind(id)
                    .fetch_one(&mut *tx)
                    .await?;
            return Ok(serde_json::from_str(&document)?);
        }
        let id = new_id();
        let directory = self.root.join("projects").join(&id);
        for folder in [
            "文献",
            "调研",
            "成果",
            "研究记录",
            "论文",
            "PPT",
            ".mathcat/artifacts",
            ".mathcat/scratch",
        ] {
            tokio::fs::create_dir_all(directory.join(folder)).await?;
        }
        let title = input["title"]
            .as_str()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or("新研究项目");
        let created_at = now();
        let mut document = json!({
            "contract":CONTRACT,"version":VERSION,"id":id,"title":title,"problem":problem,
            "problem_version":1,"revision":1,"research_revision":1,"event_cursor":1,
            "lifecycle":"open","created_at":created_at,"updated_at":created_at,
            "workspace_path":directory.to_string_lossy(),"workspace_id":input["workspace_id"],
            "material_refs":input["material_refs"].as_array().cloned().unwrap_or_default(),
            "result_state":"unresolved","_api_receipts":{},
            "project_control":{"state":"running","revision":1,"epoch":1,"paused_run_ids":[],"pending_session_ids":[],"unknown_invocation_ids":[],"outstanding_cancellation":false},
            "project_control_operations":[],"artifact_exports":[],
            "model_selection":{"model":null,"reasoning_effort":null,"revision":0,"updated_at":null},"model_selection_operations":[],
            "problem_spec":{"math_statement":problem,"research_description":"","original_input":problem,"revision":1,"problem_version":1,"normalization_state":"pending","source":"original_input","updated_at":created_at},
            "problem_spec_history":[],"planning_proposals":[],"open_questions":[],"route_outcomes":[],"whiteboard_operations":[],
            "problem_versions":[{"version":1,"problem":problem,"created_at":created_at}],
            "nodes":[{"id":new_id(),"node_type":"problem","title":title,"body":problem,"revision":1,"problem_version":1,"work_state":"open","assurance":"unreviewed","validity":"current"}]
        });
        for field in [
            "runs",
            "sessions",
            "tasks",
            "edges",
            "candidates",
            "reviews",
            "facts",
            "commands",
            "discussions",
            "annotations",
            "memories",
            "reports",
            "previews",
            "usage",
            "artifacts",
            "views",
            "bindings",
            "interactions",
            "admissions",
        ] {
            document[field] = json!([]);
        }
        let event = json!({"contract":CONTRACT,"schema_version":1,"event_id":new_id(),"project_id":id,"seq":1,"research_revision":1,"type":"project.created","occurred_at":created_at,"payload":{"id":id}});
        sqlx::query("INSERT INTO v2_projects VALUES(?,?,1,1)")
            .bind(&id)
            .bind(document.to_string())
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO v2_events VALUES(?,1,?)")
            .bind(&id)
            .bind(event.to_string())
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO v2_create_keys VALUES(?,?,?)")
            .bind(key)
            .bind(hash)
            .bind(&id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(document)
    }

    pub async fn read(&self, project_id: &str) -> V2Result<Value> {
        let document: Option<String> =
            sqlx::query_scalar("SELECT document FROM v2_projects WHERE id=?")
                .bind(project_id)
                .fetch_optional(&self.pool)
                .await?;
        serde_json::from_str(
            &document.ok_or_else(|| V2Error::new("NOT_FOUND", "Research project not found."))?,
        )
        .map_err(Into::into)
    }

    pub async fn list_projects(&self) -> V2Result<Vec<Value>> {
        let rows: Vec<String> =
            sqlx::query_scalar("SELECT document FROM v2_projects ORDER BY id DESC")
                .fetch_all(&self.pool)
                .await?;
        rows.iter()
            .map(|v| serde_json::from_str(v).map_err(Into::into))
            .collect()
    }

    pub async fn mutate<F>(
        &self,
        project_id: &str,
        event_type: &str,
        expected_revision: Option<u64>,
        update: F,
    ) -> V2Result<Value>
    where
        F: FnOnce(&mut Value) -> V2Result<Value>,
    {
        let mut tx = self.pool.begin().await?;
        let raw: Option<String> = sqlx::query_scalar("SELECT document FROM v2_projects WHERE id=?")
            .bind(project_id)
            .fetch_optional(&mut *tx)
            .await?;
        let mut project: Value = serde_json::from_str(
            &raw.ok_or_else(|| V2Error::new("NOT_FOUND", "Research project not found."))?,
        )?;
        if expected_revision.is_some_and(|v| project["revision"].as_u64() != Some(v)) {
            return Err(V2Error::new(
                "REVISION_CONFLICT",
                "The project changed; retain the draft and reload.",
            ));
        }
        let original = project.clone();
        let result = update(&mut project)?;
        if project == original {
            return Ok(result);
        }
        validate_update(&original, &project, event_type)?;
        let revision = original["revision"].as_u64().unwrap_or(0) + 1;
        let cursor = original["event_cursor"].as_u64().unwrap_or(0) + 1;
        project["revision"] = json!(revision);
        project["research_revision"] = json!(revision);
        project["event_cursor"] = json!(cursor);
        project["updated_at"] = json!(now());
        let event = json!({"contract":CONTRACT,"schema_version":1,"event_id":new_id(),"project_id":project_id,"seq":cursor,"research_revision":revision,"type":event_type,"occurred_at":now(),"payload":result});
        sqlx::query("UPDATE v2_projects SET document=?,revision=?,cursor=? WHERE id=?")
            .bind(project.to_string())
            .bind(i64::try_from(revision).unwrap_or(i64::MAX))
            .bind(i64::try_from(cursor).unwrap_or(i64::MAX))
            .bind(project_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO v2_events VALUES(?,?,?)")
            .bind(project_id)
            .bind(i64::try_from(cursor).unwrap_or(i64::MAX))
            .bind(event.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(result)
    }

    pub async fn events(&self, project_id: &str, after: u64) -> V2Result<Vec<Value>> {
        self.read(project_id).await?;
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT document FROM v2_events WHERE project_id=? AND seq>? ORDER BY seq LIMIT 1000",
        )
        .bind(project_id)
        .bind(i64::try_from(after).unwrap_or(i64::MAX))
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|v| serde_json::from_str(v).map_err(Into::into))
            .collect()
    }

    pub async fn put_artifact(
        &self,
        project_id: &str,
        filename: &str,
        content: &[u8],
        media_type: &str,
    ) -> V2Result<Value> {
        if filename.is_empty()
            || filename.len() > 250
            || filename.contains(['/', '\\', ':'])
            || filename == "."
            || filename == ".."
        {
            return Err(V2Error::new(
                "INVALID_PATH",
                "Use a plain artifact filename, not a path.",
            ));
        }
        if content.len() > 32 * 1024 * 1024 {
            return Err(V2Error::new(
                "RESOURCE_LIMIT",
                "Artifact exceeds the 32 MB limit.",
            ));
        }
        let project = self.read(project_id).await?;
        let hash = digest(content);
        let root = PathBuf::from(text(&project, "workspace_path"));
        let directory = safe_directory(&self.root, &root.join(".mathcat/artifacts")).await?;
        let file = directory.join(&hash);
        if tokio::fs::try_exists(&file).await? {
            if digest(&tokio::fs::read(&file).await?) != hash {
                return Err(V2Error::new(
                    "ARTIFACT_CORRUPTED",
                    "Existing evidence hash mismatch.",
                ));
            }
        } else {
            let temporary = directory.join(format!("{}.tmp", new_id()));
            let mut writer = tokio::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temporary)
                .await?;
            writer.write_all(content).await?;
            writer.sync_all().await?;
            drop(writer);
            match tokio::fs::rename(&temporary, &file).await {
                Ok(()) => (),
                Err(_) if tokio::fs::try_exists(&file).await? => {
                    tokio::fs::remove_file(&temporary).await?;
                }
                Err(error) => return Err(error.into()),
            }
        }
        let artifact = json!({"id":new_id(),"name":filename,"filename":filename,"sha256":hash,"size":content.len(),"byte_length":content.len(),"media_type":media_type,"path":file.to_string_lossy(),"created_at":now(),"source_type":"model_output","assurance":"unreviewed"});
        self.mutate(project_id, "artifact.published", None, |p| {
            let items = p["artifacts"]
                .as_array_mut()
                .ok_or_else(|| V2Error::new("STATE_CORRUPTED", "artifacts"))?;
            if let Some(existing) = items
                .iter()
                .find(|v| v["sha256"] == artifact["sha256"] && v["name"] == artifact["name"])
            {
                return Ok(existing.clone());
            }
            items.push(artifact.clone());
            Ok(artifact)
        })
        .await
    }

    pub async fn read_artifact(
        &self,
        project_id: &str,
        artifact_id: &str,
    ) -> V2Result<(Value, Vec<u8>)> {
        let project = self.read(project_id).await?;
        let artifact = array(&project, "artifacts")
            .into_iter()
            .find(|v| text(v, "id") == artifact_id)
            .ok_or_else(|| V2Error::new("NOT_FOUND", "Artifact not registered in this project."))?;
        let file = tokio::fs::canonicalize(text(&artifact, "path")).await?;
        let expected = self
            .root
            .join("projects")
            .join(project_id)
            .join(".mathcat/artifacts");
        let expected = tokio::fs::canonicalize(expected).await?;
        if !file.starts_with(&expected) || !expected.starts_with(&self.root) {
            return Err(V2Error::new(
                "WORKSPACE_DENIED",
                "Artifact escaped its project.",
            ));
        }
        let bytes = tokio::fs::read(file).await?;
        if digest(&bytes) != text(&artifact, "sha256") {
            return Err(V2Error::new(
                "ARTIFACT_CORRUPTED",
                "Evidence changed after publication.",
            ));
        }
        Ok((artifact, bytes))
    }

    pub async fn get_view_preferences(&self, project: &str, actor: &str) -> V2Result<Value> {
        self.read(project).await?;
        let raw: Option<String> =
            sqlx::query_scalar("SELECT document FROM v2_views WHERE project_id=? AND actor=?")
                .bind(project)
                .bind(actor)
                .fetch_optional(&self.pool)
                .await?;
        raw.map_or_else(
            || Ok(json!({"revision":0,"positions":{},"filters":{},"collapsed_ids":[]})),
            |v| Ok(serde_json::from_str(&v)?),
        )
    }

    pub async fn set_view_preferences(
        &self,
        project: &str,
        actor: &str,
        input: Value,
        key: &str,
    ) -> V2Result<Value> {
        self.read(project).await?;
        if key.is_empty() {
            return Err(V2Error::new(
                "IDEMPOTENCY_REQUIRED",
                "View updates need an idempotency key.",
            ));
        }
        let hash = digest(&serde_json::to_vec(&input)?);
        let mut tx = self.pool.begin().await?;
        if let Some(row)=sqlx::query("SELECT request_hash,response FROM v2_view_keys WHERE project_id=? AND actor=? AND key=?").bind(project).bind(actor).bind(key).fetch_optional(&mut *tx).await? {
            if row.get::<String,_>("request_hash")!=hash { return Err(V2Error::new("IDEMPOTENCY_CONFLICT","View key payload changed.")); }
            return Ok(serde_json::from_str(&row.get::<String,_>("response"))?);
        }
        let old: Option<String> =
            sqlx::query_scalar("SELECT document FROM v2_views WHERE project_id=? AND actor=?")
                .bind(project)
                .bind(actor)
                .fetch_optional(&mut *tx)
                .await?;
        let old: Value = serde_json::from_str(old.as_deref().unwrap_or("{}"))?;
        let revision = old["revision"].as_u64().unwrap_or(0);
        if input["expected_revision"].as_u64() != Some(revision) {
            return Err(V2Error::new(
                "REVISION_CONFLICT",
                "View preferences changed.",
            ));
        }
        let mut output = input;
        output
            .as_object_mut()
            .ok_or_else(|| V2Error::new("INVALID_INPUT", "View must be an object."))?
            .remove("expected_revision");
        output["revision"] = json!(revision + 1);
        sqlx::query("INSERT INTO v2_views VALUES(?,?,?) ON CONFLICT(project_id,actor) DO UPDATE SET document=excluded.document").bind(project).bind(actor).bind(output.to_string()).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO v2_view_keys VALUES(?,?,?,?,?)")
            .bind(project)
            .bind(actor)
            .bind(key)
            .bind(hash)
            .bind(output.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(output)
    }

    /// Exact evidence remains accessible even when a short retrieval result is returned.
    pub async fn memory_search(&self, project_id: &str, query: &str) -> V2Result<Vec<Value>> {
        let project = self.read(project_id).await?;
        let mut terms: Vec<String> = query
            .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .map(str::to_lowercase)
            .filter(|s| !s.is_empty())
            .collect();
        terms.extend(
            query
                .chars()
                .filter(|c| ('\u{3400}'..='\u{9fff}').contains(c))
                .map(|c| c.to_string()),
        );
        terms.sort();
        terms.dedup();
        let mut results: Vec<(usize, Value)> = Vec::new();
        for field in [
            "facts",
            "memories",
            "nodes",
            "candidates",
            "reviews",
            "artifacts",
        ] {
            for value in array(&project, field) {
                if !value["problem_version"].is_null()
                    && value["problem_version"] != project["problem_version"]
                {
                    continue;
                }
                if value["validity"].as_str().is_some_and(|v| v != "current") {
                    continue;
                }
                let body = value.to_string().to_lowercase();
                let score = terms
                    .iter()
                    .filter(|term| body.contains(term.as_str()))
                    .count();
                if terms.is_empty() || score > 0 {
                    let mut entry = value;
                    entry["memory_collection"] = json!(field);
                    entry["relevance_matches"] = json!(score);
                    for key in ["text", "body", "claim"] {
                        if let Some(body) = entry[key].as_str() {
                            if body.chars().count() > 2000 {
                                entry[key] = json!(body.chars().take(2000).collect::<String>());
                                entry["excerpt"] = json!(true);
                            }
                        }
                    }
                    results.push((score * 10 + usize::from(field == "facts"), entry));
                }
            }
        }
        results.sort_by_key(|a| std::cmp::Reverse(a.0));
        Ok(results.into_iter().take(30).map(|(_, v)| v).collect())
    }

    /// Natural-language admission is NEVER formal certification. All effective reviews count.
    pub async fn admit_candidate(&self, project_id: &str, candidate_id: &str) -> V2Result<Value> {
        let snapshot = self.read(project_id).await?;
        let candidate = array(&snapshot, "candidates")
            .into_iter()
            .find(|c| text(c, "id") == candidate_id)
            .ok_or_else(|| V2Error::new("NOT_FOUND", "Candidate not found."))?;
        self.read_artifact(project_id, text(&candidate, "proof_artifact_id"))
            .await?;
        self.mutate(project_id,"fact.admitted",None,|p|{
            let candidate=array(p,"candidates").into_iter().find(|v|text(v,"id")==candidate_id).ok_or_else(||V2Error::new("NOT_FOUND","Candidate not found."))?;
            if candidate["problem_version"]!=p["problem_version"] { return Err(V2Error::new("STALE_PROBLEM_VERSION","Candidate belongs to an older question.")); }
            let run=array(p,"runs").into_iter().find(|r|r["id"]==candidate["run_id"]).ok_or_else(||V2Error::new("INVALID_CANDIDATE","Run not found."))?;
            let within_deadline = match run.get("deadline_at") {
                Some(Value::Null) => run.get("limits").and_then(|limits| limits.get("duration_seconds")) == Some(&Value::Null),
                Some(Value::String(deadline)) => !deadline_reached(deadline),
                _ => false,
            };
            if !matches!(text(&run,"state"),"running"|"waiting_human") || !within_deadline { return Err(V2Error::new("RUN_NOT_ACTIVE","Cannot admit results after pause/stop/deadline or with an invalid deadline.")); }
            if candidate["control_epoch"]!=run["control_epoch"] { return Err(V2Error::new("STALE_CONTROL_EPOCH","Candidate belongs to an old execution epoch.")); }
            if text(&candidate,"snapshot_hash").is_empty() || text(&candidate,"author_session_id").is_empty() { return Err(V2Error::new("INVALID_CANDIDATE","Missing frozen snapshot or author.")); }
            let reviews:Vec<Value>=array(p,"reviews").into_iter().filter(|v|text(v,"candidate_id")==candidate_id && v["snapshot_hash"]==candidate["snapshot_hash"] && !matches!(text(v,"state"),"stale"|"cancelled"|"failed")).collect();
            if reviews.iter().any(|v|text(v,"state")=="completed" && matches!(text(v,"verdict"),"rejected"|"changes_requested")) { return Err(V2Error::new("REVIEW_DISPUTED","An unresolved negative review blocks admission.")); }
            let accepted:Vec<Value>=reviews.iter().filter(|r|text(r,"state")=="completed" && text(r,"verdict")=="accepted" && !text(r,"reviewer_session_id").is_empty() && r["reviewer_session_id"]!=candidate["author_session_id"]).cloned().collect();
            if accepted.is_empty() { return Err(V2Error::new("REVIEW_REQUIRED","An independent matching review is required.")); }
            if matches!(text(&candidate,"verification_engine"),"rethlas-adapted/2.1.0"|"rethlas-adapted/2.2.0") && accepted.iter().any(|r|r["report_validated"]!=true||r["engine"]!=candidate["verification_engine"]||r["packet_artifact_id"].is_null()) {return Err(V2Error::new("REVIEW_REQUIRED","Strict report and frozen evidence packet are required."));}
            if reviews.iter().any(|r|matches!(text(r,"state"),"queued"|"running")) { return Err(V2Error::new("REVIEW_PENDING","Required review is still in flight.")); }
            if candidate["declared_premises"].as_array().is_some_and(|items|items.iter().any(|p|p["kind"]=="conditional")) {return Err(V2Error::new("DEPENDENCY_UNTRUSTED","Conditional premises must be replaced by admitted facts and independently reviewed in a new snapshot."));}
            let dependencies=candidate["dependency_ids"].as_array().or_else(||candidate["dependencies"].as_array()).cloned().unwrap_or_default();
            for dep in &dependencies {
                if !dep.is_string() || !array(p,"facts").iter().any(|f|f["id"]==*dep && text(f,"validity")=="current" && f["problem_version"]==p["problem_version"]) { return Err(V2Error::new("DEPENDENCY_UNTRUSTED","A dependency is absent, challenged, revoked or belongs to an older problem.")); }
            }
            let artifact_id=text(&candidate,"proof_artifact_id");
            if let Some(fact)=p["facts"].as_array_mut().and_then(|items|items.iter_mut().find(|v|text(v,"candidate_id")==candidate_id && text(v,"validity")=="current")) {
                fact["review_ids"]=json!(accepted.iter().map(|r|r["id"].clone()).collect::<Vec<_>>());return Ok(fact.clone());
            }
            let proof=array(p,"artifacts").into_iter().find(|v|text(v,"id")==artifact_id).ok_or_else(||V2Error::new("INVALID_CANDIDATE","Proof artifact is not registered."))?;
            let fact=json!({"id":new_id(),"candidate_id":candidate_id,"run_id":run["id"],"claim":candidate["claim"],"proof_artifact_id":artifact_id,"proof_sha256":proof["sha256"],"snapshot_hash":candidate["snapshot_hash"],"problem_version":p["problem_version"],"assurance":"model_reviewed","validity":"current","revision":1,"review_ids":accepted.iter().map(|r|r["id"].clone()).collect::<Vec<_>>(),"dependency_ids":dependencies,"created_at":now()});
            p["facts"].as_array_mut().ok_or_else(||V2Error::new("STATE_CORRUPTED","facts"))?.push(fact.clone());
            if let Some(c)=p["candidates"].as_array_mut().and_then(|items|items.iter_mut().find(|v|text(v,"id")==candidate_id)) { c["status"]=json!("accepted"); }
            p["admissions"].as_array_mut().ok_or_else(||V2Error::new("STATE_CORRUPTED","admissions"))?.push(json!({"id":new_id(),"candidate_id":candidate_id,"fact_id":fact["id"],"all_effective_review_ids":reviews.iter().map(|r|r["id"].clone()).collect::<Vec<_>>(),"created_at":now()}));
            Ok(fact)
        }).await
    }

    /// Export a verified immutable byte stream into the user's organized result directories.
    pub async fn export_artifact(
        &self,
        project_id: &str,
        artifact_id: &str,
        folder: &str,
        filename: &str,
    ) -> V2Result<PathBuf> {
        if !["文献", "调研", "成果", "研究记录", "论文", "PPT"].contains(&folder)
            || filename.is_empty()
            || filename.contains(['/', '\\', ':'])
            || filename == "."
            || filename == ".."
        {
            return Err(V2Error::new(
                "INVALID_PATH",
                "Invalid result folder or filename.",
            ));
        }
        let (artifact, bytes) = self.read_artifact(project_id, artifact_id).await?;
        let project = self.read(project_id).await?;
        let workspace =
            safe_directory(&self.root, &PathBuf::from(text(&project, "workspace_path"))).await?;
        let requested = workspace.join(folder);
        if !tokio::fs::try_exists(&requested).await? {
            tokio::fs::create_dir(&requested).await?;
        }
        let directory = safe_directory(&self.root, &requested).await?;
        if !directory.starts_with(&workspace) {
            return Err(V2Error::new(
                "WORKSPACE_DENIED",
                "Result folder escaped this project workspace.",
            ));
        }
        let destination = directory.join(filename);
        if tokio::fs::try_exists(&destination).await? {
            if tokio::fs::symlink_metadata(&destination)
                .await?
                .file_type()
                .is_symlink()
            {
                return Err(V2Error::new(
                    "WORKSPACE_DENIED",
                    "Refusing a linked result file.",
                ));
            }
            if tokio::fs::read(&destination).await? == bytes {
                self.record_artifact_export(project_id, &artifact, folder, filename, &destination)
                    .await?;
                return Ok(destination);
            }
            return Err(V2Error::new(
                "FILE_EXISTS",
                "A different result already has this filename; use a new version.",
            ));
        }
        let mut writer = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)
            .await?;
        writer.write_all(&bytes).await?;
        writer.sync_all().await?;
        self.record_artifact_export(project_id, &artifact, folder, filename, &destination)
            .await?;
        Ok(destination)
    }
    async fn record_artifact_export(
        &self,
        project: &str,
        artifact: &Value,
        folder: &str,
        filename: &str,
        destination: &Path,
    ) -> V2Result<()> {
        self.mutate(project,"artifact.exported",None,|state| {
            let records=state.as_object_mut().ok_or_else(||V2Error::new("INVALID_STATE","Invalid project"))?.entry("artifact_exports").or_insert_with(||json!([])).as_array_mut().ok_or_else(||V2Error::new("INVALID_STATE","Invalid exports"))?;
            if let Some(old)=records.iter().find(|r|r["folder"]==folder&&r["filename"]==filename&&r["artifact_id"]==artifact["id"]) {return Ok(old.clone());}
            let record=json!({"id":new_id(),"artifact_id":artifact["id"],"sha256":artifact["sha256"],"folder":folder,"filename":filename,"path":destination.to_string_lossy(),"media_type":artifact["media_type"],"created_at":now()});records.push(record.clone());Ok(record)
        }).await?;
        Ok(())
    }
}

async fn safe_directory(root: &Path, path: &Path) -> V2Result<PathBuf> {
    if path.components().any(|p| matches!(p, Component::ParentDir)) {
        return Err(V2Error::new(
            "INVALID_PATH",
            "Parent traversal is not allowed.",
        ));
    }
    let canonical = tokio::fs::canonicalize(path).await?;
    if !canonical.starts_with(root) {
        return Err(V2Error::new(
            "WORKSPACE_DENIED",
            "Directory escaped data root.",
        ));
    }
    Ok(canonical)
}

fn validate_update(before: &Value, after: &Value, event_type: &str) -> V2Result<()> {
    for field in ["id", "workspace_path", "created_at", "contract"] {
        if before[field] != after[field] {
            return Err(V2Error::new(
                "IMMUTABLE_FIELD",
                format!("{field} cannot change"),
            ));
        }
    }
    let runs = array(after, "runs");
    if runs.iter().filter(|v| text(v, "state") != "ended").count() > 1 {
        return Err(V2Error::new(
            "RUN_ALREADY_ACTIVE",
            "Only one active Run is allowed.",
        ));
    }
    for old in array(before, "runs") {
        if let Some(new) = runs.iter().find(|v| v["id"] == old["id"]) {
            if old["deadline_at"].is_string() && old["deadline_at"] != new["deadline_at"] {
                return Err(V2Error::new(
                    "IMMUTABLE_DEADLINE",
                    "A Run deadline cannot be extended; create a new Run.",
                ));
            }
            if old["state"] != new["state"] {
                let previous: RunState = serde_json::from_value(old["state"].clone())?;
                let next: RunState = serde_json::from_value(new["state"].clone())?;
                if !previous.may_transition(next) {
                    return Err(V2Error::new(
                        "INVALID_TRANSITION",
                        "Illegal Run state transition.",
                    ));
                }
            }
        } else {
            return Err(V2Error::new("IMMUTABLE_HISTORY", "Runs cannot be deleted."));
        }
    }
    for fact in array(after, "facts") {
        if !array(before, "facts").iter().any(|v| v["id"] == fact["id"])
            && event_type != "fact.admitted"
        {
            return Err(V2Error::new(
                "FACT_GATE_REQUIRED",
                "Only the independent Fact Gate may add trusted results.",
            ));
        }
        if text(&fact, "assurance") == "formally_checked" {
            return Err(V2Error::new(
                "CAPABILITY_UNSUPPORTED",
                "Formal certification is not provided by the v2 natural-language gate.",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration as ChronoDuration, Utc};
    async fn setup() -> (tempfile::TempDir, V2Store, String) {
        let tmp = tempfile::tempdir().unwrap();
        let store = V2Store::connect(
            &tmp.path().join("state_v2.sqlite"),
            &tmp.path().join("data"),
        )
        .await
        .unwrap();
        let p = store
            .create_project(
                json!({"title":"测试数学项目","problem":"证明测试命题"}),
                "create-1",
            )
            .await
            .unwrap();
        (tmp, store, text(&p, "id").into())
    }
    #[tokio::test]
    async fn complete_problem_and_creation_idempotency() {
        let (_tmp, s, _id) = setup().await;
        let input = json!({"problem":"长题面".repeat(9000)});
        let a = s.create_project(input.clone(), "long").await.unwrap();
        let b = s.create_project(input, "long").await.unwrap();
        assert_eq!(a["id"], b["id"]);
        assert_eq!(a["problem"].as_str().unwrap().chars().count(), 27000);
        assert!(
            s.create_project(json!({"problem":"不同"}), "long")
                .await
                .is_err()
        );
    }
    #[tokio::test]
    async fn transactions_events_and_noop_retries() {
        let (_tmp, s, id) = setup().await;
        s.mutate(&id, "note.created", Some(1), |p| {
            p["memories"]
                .as_array_mut()
                .unwrap()
                .push(json!({"text":"第一条"}));
            Ok(json!({"ok":true}))
        })
        .await
        .unwrap();
        assert_eq!(s.read(&id).await.unwrap()["event_cursor"], 2);
        s.mutate(&id, "retry", None, |_| Ok(json!({"ok":true})))
            .await
            .unwrap();
        assert_eq!(s.events(&id, 0).await.unwrap().len(), 2);
        let failure = s
            .mutate(&id, "failed", None, |p| {
                p["title"] = json!("should rollback");
                Err(V2Error::new("FAIL", "test"))
            })
            .await;
        assert!(failure.is_err());
        assert_ne!(s.read(&id).await.unwrap()["title"], "should rollback");
        assert!(
            s.mutate(&id, "conflict", Some(1), |_| Ok(json!({})))
                .await
                .is_err()
        );
    }
    #[tokio::test]
    async fn view_preferences_do_not_change_research_revision() {
        let (_tmp, s, id) = setup().await;
        let view = json!({"expected_revision":0,"positions":{"n":{"x":1,"y":2}}});
        let a = s
            .set_view_preferences(&id, "local", view.clone(), "view-1")
            .await
            .unwrap();
        let b = s
            .set_view_preferences(&id, "local", view, "view-1")
            .await
            .unwrap();
        assert_eq!(a, b);
        assert_eq!(s.read(&id).await.unwrap()["revision"], 1);
        assert_eq!(
            s.get_view_preferences(&id, "local").await.unwrap()["revision"],
            1
        );
    }
    #[tokio::test]
    async fn artifacts_are_immutable_and_project_scoped() {
        let (_tmp, s, id) = setup().await;
        let a = s
            .put_artifact(&id, "证明.md", b"proof", "text/markdown")
            .await
            .unwrap();
        let (_, bytes) = s.read_artifact(&id, text(&a, "id")).await.unwrap();
        assert_eq!(bytes, b"proof");
        assert!(
            s.put_artifact(&id, "../escape", b"x", "text/plain")
                .await
                .is_err()
        );
        let other = s
            .create_project(json!({"problem":"other"}), "other")
            .await
            .unwrap();
        assert!(
            s.read_artifact(text(&other, "id"), text(&a, "id"))
                .await
                .is_err()
        );
        tokio::fs::write(text(&a, "path"), b"tampered")
            .await
            .unwrap();
        assert_eq!(
            s.read_artifact(&id, text(&a, "id")).await.unwrap_err().code,
            "ARTIFACT_CORRUPTED"
        );
    }
    #[tokio::test]
    async fn no_deadline_extension_or_terminal_revival() {
        let (_tmp, s, id) = setup().await;
        s.mutate(&id, "run.started", None, |p| {
            p["runs"] = json!([{"id":"r","state":"running","deadline_at":"2026-01-01T00:00:00Z"}]);
            Ok(json!({}))
        })
        .await
        .unwrap();
        assert!(
            s.mutate(&id, "extend", None, |p| {
                p["runs"][0]["deadline_at"] = json!("2027-01-01T00:00:00Z");
                Ok(json!({}))
            })
            .await
            .is_err()
        );
        s.mutate(&id, "run.ended", None, |p| {
            p["runs"][0]["state"] = json!("ended");
            Ok(json!({}))
        })
        .await
        .unwrap();
        assert!(
            s.mutate(&id, "resume", None, |p| {
                p["runs"][0]["state"] = json!("running");
                Ok(json!({}))
            })
            .await
            .is_err()
        );
    }
    async fn candidate(s: &V2Store, id: &str) {
        candidate_with_deadline(
            s,
            id,
            Some(json!((Utc::now() + ChronoDuration::hours(1)).to_rfc3339())),
            false,
        )
        .await;
    }
    async fn candidate_with_deadline(
        s: &V2Store,
        id: &str,
        deadline: Option<Value>,
        unlimited: bool,
    ) {
        let artifact = s
            .put_artifact(id, "proof.md", b"precise proof", "text/markdown")
            .await
            .unwrap();
        s.mutate(id,"candidate.submitted",None,|p|{
            let mut run=json!({"id":"r","state":"running","control_epoch":1});
            if let Some(deadline)=deadline {run["deadline_at"]=deadline;}
            if unlimited {run["limits"]=json!({"duration_seconds":null});}
            p["runs"]=json!([run]);
            p["candidates"]=json!([{"id":"c","run_id":"r","problem_version":1,"control_epoch":1,"author_session_id":"author","snapshot_hash":"frozen","proof_artifact_id":artifact["id"],"claim":"test","status":"submitted"}]);
            p["reviews"]=json!([{"id":"review","candidate_id":"c","reviewer_session_id":"independent","snapshot_hash":"frozen","state":"completed","verdict":"accepted"}]);Ok(json!({}))
        }).await.unwrap();
    }
    #[tokio::test]
    async fn fact_gate_accepts_only_explicit_unlimited_deadlines() {
        let (_tmp, s, id) = setup().await;
        candidate_with_deadline(&s, &id, Some(Value::Null), true).await;
        let packet = s
            .put_artifact(&id, "packet.json", b"{}", "application/json")
            .await
            .unwrap();
        s.mutate(&id, "review.contract", None, |p| {
            p["candidates"][0]["verification_engine"] = json!("rethlas-adapted/2.2.0");
            p["candidates"][0]["declared_premises"] = json!([]);
            p["reviews"][0]["report_validated"] = json!(true);
            p["reviews"][0]["engine"] = json!("rethlas-adapted/2.2.0");
            p["reviews"][0]["packet_artifact_id"] = packet["id"].clone();
            Ok(Value::Null)
        })
        .await
        .unwrap();
        assert_eq!(
            s.admit_candidate(&id, "c").await.unwrap()["assurance"],
            "model_reviewed"
        );
        s.mutate(&id, "test.wrong_epoch", None, |p| {
            p["runs"][0]["control_epoch"] = json!(2);
            Ok(Value::Null)
        })
        .await
        .unwrap();
        assert_eq!(
            s.admit_candidate(&id, "c").await.unwrap_err().code,
            "STALE_CONTROL_EPOCH"
        );
    }
    #[tokio::test]
    async fn fact_gate_rejects_expired_missing_or_invalid_deadlines() {
        for (deadline, unlimited) in [
            (None, true),
            (Some(Value::Null), false),
            (Some(json!("")), true),
            (Some(json!("not-a-date")), true),
            (Some(json!(5)), true),
            (
                Some(json!(
                    (Utc::now() - ChronoDuration::minutes(1)).to_rfc3339()
                )),
                true,
            ),
        ] {
            let (_tmp, s, id) = setup().await;
            candidate_with_deadline(&s, &id, deadline, unlimited).await;
            assert_eq!(
                s.admit_candidate(&id, "c").await.unwrap_err().code,
                "RUN_NOT_ACTIVE"
            );
            assert!(
                s.read(&id).await.unwrap()["facts"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
        }
    }
    #[tokio::test]
    async fn independent_gate_rejects_cherry_picked_reviews() {
        let (_tmp, s, id) = setup().await;
        candidate(&s, &id).await;
        s.mutate(&id,"review.completed",None,|p|{p["reviews"].as_array_mut().unwrap().push(json!({"id":"negative","candidate_id":"c","reviewer_session_id":"other","snapshot_hash":"frozen","state":"completed","verdict":"rejected"}));Ok(json!({}))}).await.unwrap();
        assert_eq!(
            s.admit_candidate(&id, "c").await.unwrap_err().code,
            "REVIEW_DISPUTED"
        );
    }
    #[tokio::test]
    async fn version_22_gate_requires_validated_matching_report_and_packet() {
        let (_tmp, s, id) = setup().await;
        candidate(&s, &id).await;
        s.mutate(&id, "candidate.versioned", None, |p| {
            p["candidates"][0]["verification_engine"] = json!("rethlas-adapted/2.2.0");
            p["candidates"][0]["declared_premises"] = json!([]);
            Ok(json!({}))
        })
        .await
        .unwrap();
        assert_eq!(
            s.admit_candidate(&id, "c").await.unwrap_err().code,
            "REVIEW_REQUIRED"
        );
        let packet = s
            .put_artifact(&id, "packet.json", b"{}", "application/json")
            .await
            .unwrap();
        for (validated, engine, packet_id) in [
            (false, "rethlas-adapted/2.2.0", packet["id"].clone()),
            (true, "rethlas-adapted/2.1.0", packet["id"].clone()),
            (true, "rethlas-adapted/2.2.0", Value::Null),
        ] {
            s.mutate(&id, "review.contract", None, |p| {
                p["reviews"][0]["report_validated"] = json!(validated);
                p["reviews"][0]["engine"] = json!(engine);
                p["reviews"][0]["packet_artifact_id"] = packet_id;
                Ok(json!({}))
            })
            .await
            .unwrap();
            assert_eq!(
                s.admit_candidate(&id, "c").await.unwrap_err().code,
                "REVIEW_REQUIRED"
            );
        }
        s.mutate(&id, "review.contract", None, |p| {
            p["reviews"][0]["report_validated"] = json!(true);
            p["reviews"][0]["engine"] = json!("rethlas-adapted/2.2.0");
            p["reviews"][0]["packet_artifact_id"] = packet["id"].clone();
            Ok(json!({}))
        })
        .await
        .unwrap();
        assert_eq!(
            s.admit_candidate(&id, "c").await.unwrap()["assurance"],
            "model_reviewed"
        );
    }

    #[tokio::test]
    async fn accepted_review_cannot_admit_unresolved_conditional_premises() {
        let (_tmp, s, id) = setup().await;
        candidate(&s, &id).await;
        let packet = s
            .put_artifact(&id, "packet.json", b"{}", "application/json")
            .await
            .unwrap();
        s.mutate(&id, "review.contract", None, |p| {
            p["candidates"][0]["verification_engine"] = json!("rethlas-adapted/2.2.0");
            p["candidates"][0]["declared_premises"] =
                json!([{"kind":"conditional","ref_id":"pending-lemma","usage_location":"step 3"}]);
            p["reviews"][0]["report_validated"] = json!(true);
            p["reviews"][0]["engine"] = json!("rethlas-adapted/2.2.0");
            p["reviews"][0]["packet_artifact_id"] = packet["id"].clone();
            Ok(json!({}))
        })
        .await
        .unwrap();
        assert_eq!(
            s.admit_candidate(&id, "c").await.unwrap_err().code,
            "DEPENDENCY_UNTRUSTED"
        );
        assert!(
            s.read(&id).await.unwrap()["facts"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
    #[tokio::test]
    async fn existing_fact_does_not_bypass_later_negative_review() {
        let (_tmp, s, id) = setup().await;
        candidate(&s, &id).await;
        s.admit_candidate(&id, "c").await.unwrap();
        s.mutate(&id, "review.completed", None, |p| {
            p["reviews"].as_array_mut().unwrap().push(json!({"id":"later-negative","candidate_id":"c","reviewer_session_id":"other","snapshot_hash":"frozen","state":"completed","verdict":"rejected"}));
            Ok(json!({}))
        }).await.unwrap();
        assert_eq!(
            s.admit_candidate(&id, "c").await.unwrap_err().code,
            "REVIEW_DISPUTED"
        );
    }
    #[tokio::test]
    async fn candidate_is_not_a_fact_and_review_is_not_formal() {
        let (_tmp, s, id) = setup().await;
        candidate(&s, &id).await;
        assert!(
            s.mutate(&id, "model.output", None, |p| {
                p["facts"] = json!([{"id":"forged"}]);
                Ok(json!({}))
            })
            .await
            .is_err()
        );
        let f = s.admit_candidate(&id, "c").await.unwrap();
        assert_eq!(f["assurance"], "model_reviewed");
        assert_eq!(s.admit_candidate(&id, "c").await.unwrap()["id"], f["id"]);
    }
    #[tokio::test]
    async fn gate_rejects_self_review_and_expired_run() {
        let (_tmp, s, id) = setup().await;
        candidate(&s, &id).await;
        s.mutate(&id, "review.completed", None, |p| {
            p["reviews"][0]["reviewer_session_id"] = json!("author");
            Ok(json!({}))
        })
        .await
        .unwrap();
        assert_eq!(
            s.admit_candidate(&id, "c").await.unwrap_err().code,
            "REVIEW_REQUIRED"
        );
        s.mutate(&id, "run.ended", None, |p| {
            p["runs"][0]["state"] = json!("ended");
            Ok(json!({}))
        })
        .await
        .unwrap();
        assert_eq!(
            s.admit_candidate(&id, "c").await.unwrap_err().code,
            "RUN_NOT_ACTIVE"
        );
    }
    #[tokio::test]
    async fn memories_include_failures_without_promoting_them() {
        let (_tmp, s, id) = setup().await;
        s.mutate(&id,"memory.created",None,|p|{p["memories"]=json!([{"id":"m","text":"积分交换缺少条件","kind":"failed_attempt","validity":"current","assurance":"unreviewed"}]);Ok(json!({}))}).await.unwrap();
        let hits = s.memory_search(&id, "积分").await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0]["assurance"], "unreviewed");
    }
    #[tokio::test]
    async fn reopen_recovers_exact_state_and_events() {
        let (tmp, s, id) = setup().await;
        s.mutate(&id, "note.created", None, |p| {
            p["title"] = json!("持久状态");
            Ok(json!({}))
        })
        .await
        .unwrap();
        let reopened = V2Store::connect(
            &tmp.path().join("state_v2.sqlite"),
            &tmp.path().join("data"),
        )
        .await
        .unwrap();
        assert_eq!(reopened.read(&id).await.unwrap()["title"], "持久状态");
        assert_eq!(reopened.events(&id, 1).await.unwrap().len(), 1);
    }
    #[tokio::test]
    async fn result_exports_are_organized_and_never_overwrite_different_content() {
        let (_tmp, s, id) = setup().await;
        let a = s
            .put_artifact(&id, "report.md", b"first report", "text/markdown")
            .await
            .unwrap();
        let exported = s
            .export_artifact(&id, text(&a, "id"), "成果", "研究报告.md")
            .await
            .unwrap();
        assert_eq!(tokio::fs::read(&exported).await.unwrap(), b"first report");
        assert_eq!(
            s.export_artifact(&id, text(&a, "id"), "成果", "研究报告.md")
                .await
                .unwrap(),
            exported
        );
        let b = s
            .put_artifact(&id, "report-v2.md", b"new report", "text/markdown")
            .await
            .unwrap();
        assert_eq!(
            s.export_artifact(&id, text(&b, "id"), "成果", "研究报告.md")
                .await
                .unwrap_err()
                .code,
            "FILE_EXISTS"
        );
        assert!(
            s.export_artifact(&id, text(&a, "id"), "../old", "x.md")
                .await
                .is_err()
        );
        assert!(
            s.export_artifact(&id, text(&a, "id"), "成果", "../x.md")
                .await
                .is_err()
        );
        let before = s.read_artifact(&id, text(&a, "id")).await.unwrap();
        let record = s
            .export_artifact(&id, text(&a, "id"), "研究记录", "冻结草稿.md")
            .await
            .unwrap();
        assert_eq!(tokio::fs::read(record).await.unwrap(), before.1);
        s.export_artifact(&id, text(&a, "id"), "研究记录", "冻结草稿.md")
            .await
            .unwrap();
        let after = s.read(&id).await.unwrap();
        let exports = array(&after, "artifact_exports")
            .into_iter()
            .filter(|r| r["folder"] == "研究记录")
            .collect::<Vec<_>>();
        assert_eq!(exports.len(), 1);
        assert_eq!(exports[0]["sha256"], a["sha256"]);
        assert_eq!(exports[0]["artifact_id"], a["id"]);
        assert_eq!(s.read_artifact(&id, text(&a, "id")).await.unwrap(), before);
    }
    #[tokio::test]
    async fn gate_checks_dependency_validity_and_problem_version() {
        let (_tmp, s, id) = setup().await;
        candidate(&s, &id).await;
        s.mutate(&id, "candidate.updated", None, |p| {
            p["candidates"][0]["dependency_ids"] = json!(["missing"]);
            Ok(json!({}))
        })
        .await
        .unwrap();
        assert!(s.admit_candidate(&id, "c").await.is_err());
        s.mutate(&id, "candidate.updated", None, |p| {
            p["candidates"][0]["dependency_ids"] = json!([]);
            p["candidates"][0]["problem_version"] = json!(2);
            Ok(json!({}))
        })
        .await
        .unwrap();
        assert!(s.admit_candidate(&id, "c").await.is_err());
        assert!(
            s.read(&id).await.unwrap()["facts"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
    #[tokio::test]
    async fn gate_rechecks_proof_bytes_before_admission() {
        let (_tmp, s, id) = setup().await;
        candidate(&s, &id).await;
        let p = s.read(&id).await.unwrap();
        tokio::fs::write(text(&p["artifacts"][0], "path"), b"different proof")
            .await
            .unwrap();
        assert_eq!(
            s.admit_candidate(&id, "c").await.unwrap_err().code,
            "ARTIFACT_CORRUPTED"
        );
        assert!(
            s.read(&id).await.unwrap()["facts"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
}
