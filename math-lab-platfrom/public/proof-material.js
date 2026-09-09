// Follow only an explicit, registered reference frozen in this object's body artifact.
export function registeredBodyDraft(artifact,project){
  if(typeof artifact?.text!=='string')return null;
  let body;try{body=JSON.parse(artifact.text);}catch{return null;}
  if(!Array.isArray(body?.draft_artifact_refs))return null;
  const ref=body.draft_artifact_refs.at(-1),id=ref?.artifact_id||ref?.id;
  const registered=(project.artifacts||[]).find(a=>a.id===id);
  return registered&&(!ref.sha256||ref.sha256===registered.sha256)?id:null;
}
