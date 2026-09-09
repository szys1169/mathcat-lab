import test from "node:test";
import assert from "node:assert/strict";
import { extractLatexSegments, renderLatexText } from "../public/math-renderer.js";
import katex from "katex";

const escapeHtml = (value) => String(value).replace(/[&<>"']/g, (char) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[char]));

test('nested and multi-digit scripts preserve their complete mathematical scope',()=>{
  const sources=[];
  const html=renderLatexText("H_((n+1)/2)(n) C_1^u_1 x_{i_{j_k}} a_12 2a_i^2 (F')^3 eta^2 L^*",{
    escapeHtml,renderFormula:s=>{sources.push(s);return katex.renderToString(s,{throwOnError:true,strict:'ignore'});}
  });
  assert.deepEqual(sources,['H_{(n+1)/2}','C_{1}^{u_{1}}','x_{i_{j_k}}','a_{12}','a_{i}^{2}',"(F')^{3}",'\\eta^{2}','L^{*}']);
  assert.doesNotMatch(html,/math-fallback/);
});

test('unbalanced scripts and ordinary paths remain readable without partial repair',()=>{
  const html=renderLatexText('x_{i+1 `C_1^u_1` file_name /a/J_f F:\\work\\J_f full_proof.md',{
    escapeHtml,renderFormula:()=>{throw Error('must not infer math here');}
  });
  assert.doesNotMatch(html,/math-fallback/);
  assert.match(html,/x_\{i\+1/);
  assert.match(html,/<code>C_1\^u_1<\/code>/);
});

test('result prose renders Markdown and isolated subscripts while protecting code and paths',()=>{
  const renderFormula=(source)=>katex.renderToString(source,{throwOnError:true});
  const html=renderLatexText('# 结论\n**关键结论**：J_f 与 J\\_f、x_{i}、x^2。\n- 保留假设\n`J_f **literal**`\n```js\nconst file_name = "J_f";\n```\nfile_name full_proof.md /a/J_f',{escapeHtml,renderFormula});
  assert.match(html,/<strong>关键结论<\/strong>/);assert.match(html,/role="heading"/);
  assert.equal((html.match(/class="katex"/g)||[]).length,4);
  assert.match(html,/<code>J_f \*\*literal\*\*<\/code>/);assert.match(html,/file_name full_proof.md \/a\/J_f/);
  assert.match(html,/class="md-code-block"/);
});

test('formatting escapes HTML and does not treat explicit formulas as Markdown',()=>{
  const html=renderLatexText('**<img src=x onerror=alert(1)>** $J_f$ \\*literal\\*',{escapeHtml,renderFormula:source=>'<math>'+escapeHtml(source)+'</math>'});
  assert.doesNotMatch(html,/<img/);assert.match(html,/<math>J_f<\/math>/);assert.match(html,/\*literal\*/);
});

test('actual human-mode reviewer notation and leader assignment table render completely',()=>{
  const text='m^(N+2) J_{σp} k⊗_F B C⊗_{F,σ}B Σ_{r≥1}b_r(f) ∏_i x_i^(a_i−2) x_i^a_i ∂_iP R/J_f Ann_A(a) dim_k Mat_τ';
  const html=renderLatexText(text,{escapeHtml,renderFormula:s=>katex.renderToString(s,{throwOnError:true,strict:'ignore'})});
  assert.doesNotMatch(html,/math-fallback/);assert.equal((html.match(/class="katex"/g)||[]).length,14);
  const table=renderLatexText('建议分工如下：\n\n| 角色 | 任务 |\n|---|---|\n| **伙伴 A** | 证明 $J_f$ |',{escapeHtml,renderFormula:()=>'<math>J</math>'});
  assert.match(table,/role="table"/);assert.equal((table.match(/role="row"/g)||[]).length,2);assert.match(table,/<strong>伙伴 A<\/strong>/);
});

test("latex extractor supports common inline, display, and environment delimiters", () => {
  const result = extractLatexSegments("行内 \\(x^2+y^2\\)，另一个 $a+b$。\\[c=d\\] $$e=f$$ \\begin{align}g&=h\\end{align}");
  assert.equal(result.formulas.length, 5);
  assert.deepEqual(result.formulas.map((item) => item.displayMode), [false, false, true, true, true]);
});

test("latex inside code spans and fenced code blocks is not rendered", () => {
  const result = extractLatexSegments("`$not_math$`\n```tex\n\\[also_not_math\\]\n```\n\\(real_math\\)");
  assert.equal(result.formulas.length, 1);
  assert.equal(result.formulas[0].source, "real_math");
  assert.equal(result.code.length, 2);
});

test("renderer preserves text and falls back visibly when one formula is invalid", () => {
  const html = renderLatexText("安全 <text>：\\(good\\)，\\(bad\\)", {
    escapeHtml,
    renderFormula: (source) => { if (source === "bad") throw new Error("invalid"); return `<math>${source}</math>`; }
  });
  assert.match(html, /安全 &lt;text&gt;/);
  assert.match(html, /<math>good<\/math>/);
  assert.match(html, /class="math-fallback"/);
  assert.match(html, /\\\(bad\\\)/);
});

test("bundled KaTeX renders real inline and display formulas", () => {
  const html = renderLatexText("\\(x^2+y^2=z^2\\) and \\[\\int_0^1 x\\,dx=\\frac12\\]", {
    escapeHtml,
    renderFormula: (source, displayMode) => katex.renderToString(source, { displayMode, throwOnError: true })
  });
  assert.match(html, /class="katex"/);
  assert.match(html, /class="katex-display"/);
  assert.doesNotMatch(html, /math-fallback/);
});
