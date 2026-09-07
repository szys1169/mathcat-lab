-- SQLite serves its durable `events` table directly to replayable SSE/WS clients.
-- Earlier versions also left every event in a pending outbox although no external
-- dispatcher existed. Mark that historical local backlog delivered. PostgreSQL has
-- separate migrations and retains its pending transactional outbox semantics.
UPDATE event_outbox
SET status = 'delivered',
    delivered_at = COALESCE(delivered_at, available_at),
    last_error = NULL
WHERE status = 'pending';
