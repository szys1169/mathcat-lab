import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import { expectedVersion,versionRoot,backendUrl,platformUrl } from "../scripts/version-runtime.mjs";

test("cross-platform launch metadata stays version isolated",async()=>{
  assert.equal(expectedVersion,(await fs.readFile(path.join(versionRoot,"VERSION"),"utf8")).trim());
  const release=JSON.parse(await fs.readFile(path.join(versionRoot,"release.json"),"utf8"));
  assert.equal(release.version,expectedVersion);
  assert.equal(new URL(backendUrl).port,String(release.runtime.research_port));
  assert.equal(new URL(platformUrl).port,String(release.runtime.web_port));
});

test("macOS wrappers use the cross-platform Node lifecycle",async()=>{
  const start=await fs.readFile(path.join(versionRoot,"scripts","start-version.sh"),"utf8");
  const stop=await fs.readFile(path.join(versionRoot,"scripts","stop-version.sh"),"utf8");
  assert.match(start,/start-version\.mjs/);
  assert.match(stop,/stop-version\.mjs/);
  assert.doesNotMatch(start,/powershell|taskkill/i);
  assert.doesNotMatch(stop,/powershell|taskkill/i);
});
