#![cfg(feature = "postgres")]

use research_storage::PostgresStateCommitter;
use serde_json::json;

#[tokio::test]
async fn postgres_committer_leases_and_ingests_when_test_url_is_configured()
-> Result<(), Box<dyn std::error::Error>> {
    let Ok(database_url) = std::env::var("MRA_TEST_POSTGRES_URL") else {
        eprintln!("MRA_TEST_POSTGRES_URL is not set; PostgreSQL integration test skipped");
        return Ok(());
    };
    let committer = PostgresStateCommitter::connect(&database_url, 4).await?;
    let suffix = ulid::Ulid::new().to_string();
    let project_id = format!("pgtest_project_{suffix}");
    let route_id = format!("pgtest_route_{suffix}");
    let task_id = format!("pgtest_task_{suffix}");
    let worker_instance_id = format!("pgtest_worker_{suffix}");
    sqlx::query("INSERT INTO projects(project_id,revision,status,contract_json,budget_json) VALUES($1,1,'running',$2,$3)")
        .bind(&project_id)
        .bind(json!({"target_statement":"1+1=2"}))
        .bind(json!({"max_attempts":3}))
        .execute(committer.pool())
        .await?;
    sqlx::query("INSERT INTO routes(route_id,project_id,status,cancellation_epoch,semantic_fingerprint,payload_json) VALUES($1,$2,'active',0,$3,$4)")
        .bind(&route_id)
        .bind(&project_id)
        .bind(format!("fingerprint-{suffix}"))
        .bind(json!({"title":"integration"}))
        .execute(committer.pool())
        .await?;
    sqlx::query("INSERT INTO tasks(task_id,project_id,route_id,task_signature,status,priority,revision,route_cancellation_epoch,contract_json,payload_json) VALUES($1,$2,$3,$4,'queued',1.0,1,0,$5,$6)")
        .bind(&task_id)
        .bind(&project_id)
        .bind(&route_id)
        .bind(format!("signature-{suffix}"))
        .bind(json!({"completion":"one envelope"}))
        .bind(json!({"objective":"integration"}))
        .execute(committer.pool())
        .await?;

    let lease = committer
        .lease_next_task(&project_id, "integration-node", &worker_instance_id, 60)
        .await?
        .expect("queued task should be leased");
    committer.renew_lease(&lease, 60).await?;
    let payload = json!({"summary":"completed","candidates":[]});
    let first_envelope = committer
        .submit_result_envelope(&lease, "success", &payload)
        .await?;
    let duplicate_envelope = committer
        .submit_result_envelope(&lease, "success", &payload)
        .await?;
    assert_eq!(first_envelope, duplicate_envelope);
    assert_eq!(
        committer.ingest_result_envelope(&first_envelope).await?,
        payload
    );
    let status: String = sqlx::query_scalar("SELECT status FROM tasks WHERE task_id=$1")
        .bind(&task_id)
        .fetch_one(committer.pool())
        .await?;
    assert_eq!(status, "completed");
    let outbox_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM event_outbox WHERE project_id=$1")
            .bind(&project_id)
            .fetch_one(committer.pool())
            .await?;
    assert!(outbox_count >= 3);
    sqlx::query("DELETE FROM projects WHERE project_id=$1")
        .bind(&project_id)
        .execute(committer.pool())
        .await?;
    Ok(())
}
