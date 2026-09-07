// Keep the conversation list aligned with the authoritative research snapshot.
// This is presentation metadata; it never changes a run or its evidence.
export async function syncResearchConversationStatus(store, conversation, project) {
  if (!store?.getConversation || !store?.updateConversation) return;
  const current = store.getConversation(conversation.id);
  const runs = project.runs || [];
  const run = runs.at(-1);
  if (!run || !current || current.researchProjectId !== project.id) return;
  // A newly dispatched run may be newer than the snapshot already being observed.
  if (current.researchRunId && !runs.some(row => row.id === current.researchRunId)) return;
  const update = {
    status: run.state === 'ended' ? 'idle' : run.state === 'paused' ? 'paused' : 'running',
    researchRunId: run.id,
    researchRunState: run.state,
    researchResultState: run.result_state || project.result_state || 'unresolved',
    researchOutstandingCancellation: Boolean(run.outstanding_cancellation),
  };
  if (Object.entries(update).every(([key, value]) => current[key] === value)) return;
  await store.updateConversation(current.id, row => {
    if (row.researchProjectId !== project.id) return;
    Object.assign(row, update);
  });
}
