import test from "node:test";
import assert from "node:assert/strict";
import { extractLatexSegments, renderLatexText } from "../public/math-renderer.js";
import katex from "katex";

const escapeHtml = (value) => String(value).replace(/[&<>"']/g, (char) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[char]));

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
