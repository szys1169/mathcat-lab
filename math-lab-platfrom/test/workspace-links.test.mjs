import test from 'node:test';
import assert from 'node:assert/strict';
import {prepareMessageLinks,classifyMessageLink,workspaceFileUrl} from '../public/workspace-links.js';
import {renderLatexText} from '../public/math-renderer.js';
import {escapeHtml} from '../public/research-v2-state.js';

function render(text,id='conversation-a'){
  const prepared=prepareMessageLinks(text,id,(conversation,file,label='',fragment='')=>'<a href="'+escapeHtml(workspaceFileUrl(conversation,file)+fragment)+'">'+escapeHtml(label||file)+'</a>');
  return prepared.restore(renderLatexText(prepared.text,{escapeHtml,renderFormula:source=>'<math>'+escapeHtml(source)+'</math>'}));
}
test('relative deliverables and version directories link to the originating conversation workspace',()=>{
  const text='[报告](成果/文献调研/conjecture-3-6/report.md) [PDF](<论文/中文 稿.pdf#page=2>) [版本目录](论文/v3/) [原题](problem.md)';
  const html=render(text);
  assert.match(html,/conversationId=conversation-a/);assert.ok(html.includes(encodeURIComponent('成果/文献调研/conjecture-3-6/report.md')));assert.ok(html.includes(encodeURIComponent('论文/中文 稿.pdf')+'#page=2'));assert.ok(html.includes(encodeURIComponent('论文/v3/')));assert.ok(html.includes('path=problem.md'));
  assert.doesNotMatch(render(text,'conversation-b'),/conversation-a/);
});
test('external links, unsafe schemes, formulas and literal code retain correct boundaries',()=>{
  const html=render('[文献](https://arxiv.org/abs/2509.16933?x=1&y=2) [坏链接](javascript:alert(1)) `x [样例](paper.md)` \\(J_f:f\\)');
  assert.match(html,/href="https:\/\/arxiv.org\/abs\/2509.16933\?x=1&amp;y=2"/);assert.doesNotMatch(html,/href="javascript:/);assert.match(html,/`x \[样例\]\(paper.md\)`/);assert.match(html,/<math>J_f:f<\/math>/);
  assert.equal(classifyMessageLink('https://example.com/a.pdf').kind,'external');assert.equal(classifyMessageLink('data:text/html,bad').kind,'invalid');
});
test('Windows paths, percent encoded names and nested parentheses are preserved',()=>{
  const html=render('[稿件](</F:/study/论文/a (v2).pdf>) [编码](成果/%E4%B8%AD%E6%96%87.md)');
  assert.ok(html.includes(encodeURIComponent('F:/study/论文/a (v2).pdf')));assert.ok(html.includes(encodeURIComponent('成果/中文.md')));
  assert.equal(classifyMessageLink('%E0%A4%A').kind,'invalid');
});
