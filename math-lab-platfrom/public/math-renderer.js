const CODE_TOKEN = "@@MATHCAT_CODE_";
const MATH_TOKEN = "@@MATHCAT_FORMULA_";

function protectCode(value) {
  const code = [];
  const text = String(value).replace(/```[\s\S]*?```|`[^`\r\n]*`/g, (match) => {
    const token = `${CODE_TOKEN}${code.length}@@`;
    code.push(match);
    return token;
  });
  return { text, code };
}

export function extractLatexSegments(value) {
  const { text: protectedText, code } = protectCode(value);
  const formulas = [];
  const pattern = /\\begin\{(equation\*?|align\*?|aligned|gather\*?|multline\*?)\}([\s\S]*?)\\end\{\1\}|\\\[([\s\S]*?)\\\]|\$\$([\s\S]*?)\$\$|\\\(([^\r\n]*?)\\\)|(?<!\\)\$(?!\$)([^$\r\n]+?)(?<!\\)\$/g;
  const text = protectedText.replace(pattern, (match, environment, environmentBody, bracketBody, dollarBody, parenBody, inlineDollarBody) => {
    const source = environment ? `\\begin{${environment}}${environmentBody}\\end{${environment}}` : bracketBody ?? dollarBody ?? parenBody ?? inlineDollarBody ?? "";
    if (!source.trim()) return match;
    const displayMode = Boolean(environment || bracketBody != null || dollarBody != null);
    const token = `${MATH_TOKEN}${formulas.length}@@`;
    formulas.push({ source: source.trim(), displayMode, original: match });
    return token;
  });
  return { text, code, formulas };
}

export function renderLatexText(value, { escapeHtml, renderFormula }) {
  const { text, code, formulas } = extractLatexSegments(value);
  let html = escapeHtml(text);
  code.forEach((source, index) => { html = html.replace(`${CODE_TOKEN}${index}@@`, escapeHtml(source)); });
  formulas.forEach((formula, index) => {
    let rendered;
    try { rendered = renderFormula(formula.source, formula.displayMode); }
    catch { rendered = `<span class="math-fallback${formula.displayMode ? " display" : ""}" title="公式暂时无法渲染">${escapeHtml(formula.original)}</span>`; }
    html = html.replace(`${MATH_TOKEN}${index}@@`, rendered);
  });
  return html;
}
