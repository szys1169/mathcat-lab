//! Regression coverage for restored research epochs and human review interleavings.

use super::{V2Config, V2Service, V2Store, array, entity_mut, new_session, now, push};
use research_worker_runtime::research_v2::{TurnError, TurnOutput, TurnRequest, V2Backend};
use serde_json::{Value, json};
use std::{path::Path, sync::Arc};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

struct UnusedBackend;

#[async_trait::async_trait]
impl V2Backend for UnusedBackend {
    async fn preflight(&self) -> Result<Value, TurnError> {
        Err(TurnError::new(
            "UNEXPECTED_CALL",
            "state regression must not preflight a model",
        ))
    }

    async fn run_turn(
        &self,
        _request: TurnRequest,
        _events: mpsc::Sender<Value>,
        _cancel: CancellationToken,
    ) -> Result<TurnOutput, TurnError> {
        Err(TurnError::new(
            "UNEXPECTED_CALL",
            "state regression must not call a model",
        ))
    }
}

async fn fixture() -> (tempfile::TempDir, V2Service, String, String) {
    let temp = tempfile::tempdir().unwrap();
    let store = V2Store::connect(
        &temp.path().join("lifecycle.sqlite"),
        &temp.path().join("data"),
    )
    .await
    .unwrap();
    let project = store
        .create_project(
            json!({"title":"lifecycle fixture","problem":"prove the complete fixture goal"}),
            "create",
        )
        .await
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let service = V2Service::with_backend(store, V2Config::default(), Arc::new(UnusedBackend));
    let author = service.store.mutate(&project, "fixture.seed", None, |state| {
        let run = json!({"id":"run","state":"running","problem_version":1,"control_epoch":1,"started_at":now(),"deadline_at":(chrono::Utc::now()+chrono::Duration::seconds(60)).to_rfc3339(),"limits":{"max_invocations":100},"revision":1});
        push(state, "runs", run.clone());
        let author = new_session(state, &run, "partner", Some("complete fixture goal"), &None);
        push(state, "sessions", author.clone());
        Ok(author)
    }).await.unwrap()["id"].as_str().unwrap().to_owned();
    (temp, service, project, author)
}

async fn submit_same_proof(
    service: &V2Service,
    project: &str,
    author: &str,
    epoch: u64,
    action_id: &str,
    workspace: &Path,
) {
    let artifact = service
        .store
        .put_artifact(
            project,
            &format!("output-{action_id}.md"),
            b"proof submission",
            "text/markdown",
        )
        .await
        .unwrap();
    let control = json!({"actions":[{"type":"submit_candidate","action_id":action_id,"claim":"complete fixture goal","proof":"same complete fixture proof","covers_goal":true,"declared_premises":[]}]});
    service
        .apply_actions(
            project, "run", author, epoch, &control, &artifact, workspace,
        )
        .await
        .unwrap();
}

async fn independent_acceptance(service: &V2Service, project: &str, candidate_id: &str) -> Value {
    let packet = service
        .store
        .put_artifact(
            project,
            &format!("packet-{candidate_id}.json"),
            b"{}",
            "application/json",
        )
        .await
        .unwrap();
    service
        .store
        .mutate(project, "fixture.independent_review", None, |state| {
            let candidate = array(state, "candidates")
                .iter()
                .find(|candidate| candidate["id"] == candidate_id)
                .unwrap()
                .clone();
            for review in state["reviews"].as_array_mut().unwrap() {
                if review["candidate_id"] == candidate_id {
                    assert_ne!(
                        review["reviewer_session_id"],
                        candidate["author_session_id"]
                    );
                    review["state"] = json!("completed");
                    review["verdict"] = json!("accepted");
                    review["goal_coverage"] = json!(true);
                    review["report_validated"] = json!(true);
                    review["engine"] = candidate["verification_engine"].clone();
                    review["packet_artifact_id"] = packet["id"].clone();
                }
            }
            Ok(Value::Null)
        })
        .await
        .unwrap();
    service
        .store
        .admit_candidate(project, candidate_id)
        .await
        .unwrap()
}

#[tokio::test]
async fn identical_proof_after_control_epoch_change_creates_admissible_lineage_version() {
    let (temp, service, project, author) = fixture().await;
    submit_same_proof(&service, &project, &author, 1, "epoch-one", temp.path()).await;
    submit_same_proof(
        &service,
        &project,
        &author,
        1,
        "same-epoch-repeat",
        temp.path(),
    )
    .await;
    let original = service.store.read(&project).await.unwrap();
    assert_eq!(
        array(&original, "candidates").len(),
        1,
        "same-generation idempotency remains intact"
    );
    let old = original["candidates"][0].clone();
    // pause_run/resume_run preserves partner identity but advances the run control epoch.
    service
        .store
        .mutate(&project, "fixture.restore_epoch", None, |state| {
            entity_mut(state, "runs", "run")?["control_epoch"] = json!(2);
            for review in state["reviews"].as_array_mut().unwrap() {
                review["state"] = json!("cancelled");
            }
            Ok(Value::Null)
        })
        .await
        .unwrap();
    submit_same_proof(&service, &project, &author, 2, "epoch-two", temp.path()).await;
    let state = service.store.read(&project).await.unwrap();
    assert_eq!(array(&state, "candidates").len(), 2);
    let renewed = &state["candidates"][1];
    assert_ne!(renewed["id"], old["id"]);
    assert_eq!(renewed["snapshot_hash"], old["snapshot_hash"]);
    assert_eq!(renewed["lineage_id"], old["lineage_id"]);
    assert_eq!(renewed["repair_of"], old["id"]);
    assert_eq!(renewed["control_epoch"], 2);
    assert_eq!(state["candidates"][0]["control_epoch"], 1);
    let fact = independent_acceptance(&service, &project, renewed["id"].as_str().unwrap()).await;
    assert_eq!(fact["candidate_id"], renewed["id"]);
    assert_eq!(
        service
            .store
            .admit_candidate(&project, old["id"].as_str().unwrap())
            .await
            .unwrap_err()
            .code,
        "STALE_CONTROL_EPOCH"
    );
}

#[tokio::test]
async fn human_challenge_between_admission_and_goal_stop_keeps_research_running() {
    let (temp, service, project, author) = fixture().await;
    submit_same_proof(&service, &project, &author, 1, "goal", temp.path()).await;
    let candidate_id = service.store.read(&project).await.unwrap()["candidates"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let earlier_fact = independent_acceptance(&service, &project, &candidate_id).await;
    let command = json!({"id":"human-challenge","run_id":"run","type":"challenge_evidence","target":{"id":earlier_fact["id"]},"status":"queued","revision":1});
    service
        .store
        .mutate(&project, "fixture.challenge_queued", None, |state| {
            push(state, "commands", command.clone());
            Ok(Value::Null)
        })
        .await
        .unwrap();
    // Exercise the real human control handler in the gap before finish_review requests stop.
    service.apply_control(&project, &command).await.unwrap();
    assert_eq!(
        earlier_fact["validity"], "current",
        "caller still holds the admission result"
    );
    service
        .finish(&project, "run", "goal_satisfied", None)
        .await
        .unwrap();
    let current = service.store.read(&project).await.unwrap();
    assert_eq!(current["facts"][0]["validity"], "challenged");
    assert_eq!(current["runs"][0]["state"], "running");
    assert_eq!(current["runs"][0]["result_state"], "unresolved");
    assert_eq!(current["runs"][0]["control_epoch"], 1);
    assert!(current["runs"][0]["requested_stop_reason"].is_null());
    assert!(array(&current, "reports").is_empty());
    assert!(
        array(&current, "sessions")
            .iter()
            .all(|session| session["state"] != "closed")
    );
}

#[tokio::test]
async fn unchanged_admitted_goal_still_completes_the_run() {
    let (temp, service, project, author) = fixture().await;
    submit_same_proof(&service, &project, &author, 1, "goal", temp.path()).await;
    let candidate_id = service.store.read(&project).await.unwrap()["candidates"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    independent_acceptance(&service, &project, &candidate_id).await;
    service
        .finish(&project, "run", "goal_satisfied", None)
        .await
        .unwrap();
    let state = service.store.read(&project).await.unwrap();
    assert_eq!(state["runs"][0]["state"], "ended");
    assert_eq!(state["runs"][0]["stop_reason"], "goal_satisfied");
    assert_eq!(state["runs"][0]["result_state"], "reviewed_solution");
    assert_eq!(array(&state, "reports").len(), 1);
}
