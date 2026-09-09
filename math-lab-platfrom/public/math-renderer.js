import {extractBareMath} from './math-notation.js';
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
  const prose = extractBareMath(text, (original, source) => {
    const token = `${MATH_TOKEN}${formulas.length}@@`;
    formulas.push({source,displayMode:false,original});return token;
  });
  let html = renderMarkdownProse(prose, escapeHtml);
  formulas.forEach((formula, index) => {
    let rendered;
    try { rendered = renderFormula(formula.source, formula.displayMode); }
    catch { rendered = `<span class="math-fallback${formula.displayMode ? " display" : ""}" title="公式暂时无法渲染">${escapeHtml(formula.original)}</span>`; }
    html = html.replace(`${MATH_TOKEN}${index}@@`, () => rendered);
  });
  code.forEach((source, index) => {
    const fenced=source.startsWith('```');
    const body=fenced?source.replace(/^```[^\r\n]*\r?\n?/,'').replace(/```$/,''):source.slice(1,-1);
    html = html.replace(`${CODE_TOKEN}${index}@@`, () => `<code${fenced?' class="md-code-block"':''}>${escapeHtml(body)}</code>`);
  });
  return html;
}

// Format escaped prose only; generated math and code are restored afterwards.
// Spans keep this renderer usable inside both inline labels and reading blocks.
function renderMarkdownProse(text, escapeHtml) {
  const escaped=[];
  let html=escapeHtml(text.replace(/\\([\\`*{}\[\]()#+.!_>~-])/g,(_,c)=>{
    const token=`@@MATHCAT_ESCAPE_${escaped.length}@@`;escaped.push(c);return token;
  }));
  html=html.replace(/\*\*(?=\S)([^*]*?\S)\*\*/g,'<strong>$1</strong>')
    .replace(/__(?=\S)([^_]*?\S)__/g,'<strong>$1</strong>')
    .replace(/(?<!\*)\*(?![\s*])([^*\r\n]*?\S)\*(?!\*)/g,'<em>$1</em>')
    .replace(/^ {0,3}(#{1,6})[\t ]+(.+?)[\t ]*#*[\t ]*$/gm,(_,marks,body)=>`<span class="md-heading md-heading-${marks.length}" role="heading" aria-level="${marks.length}">${body}</span>`)
    .replace(/^ {0,3}&gt; ?(.+)$/gm,'<span class="md-quote">$1</span>')
    .replace(/^([\t ]*)[-+*][\t ]+(.+)$/gm,'$1<span class="md-list-item">• $2</span>')
    .replace(/^([\t ]*)(\d+)[.)][\t ]+(.+)$/gm,'$1<span class="md-list-item">$2. $3</span>');
  const lines=html.split(/\r?\n/),formatted=[];
  const cells=line=>line.trim().replace(/^\|/,'').replace(/\|$/,'').split('|').map(c=>c.trim());
  for(let i=0;i<lines.length;i++){
    const separator=lines[i+1];
    if(lines[i].includes('|')&&separator?.includes('|')&&cells(separator).every(c=>/^:?-{3,}:?$/.test(c))&&cells(lines[i]).length===cells(separator).length){
      const row=(line,header)=>'<span class="md-table-row" role="row">'+cells(line).map(c=>'<span class="md-table-cell" role="'+(header?'columnheader':'cell')+'">'+c+'</span>').join('')+'</span>';
      let table='<span class="md-table-wrap"><span class="md-table" role="table">'+row(lines[i],true);i++;
      while(i+1<lines.length&&lines[i+1].includes('|')&&lines[i+1].trim()){table+=row(lines[++i],false);}
      formatted.push(table+'</span></span>');
    }else formatted.push(lines[i]);
  }
  html=formatted.join('\n');
  escaped.forEach((c,i)=>{html=html.replace(`@@MATHCAT_ESCAPE_${i}@@`,()=>escapeHtml(c));});
  return html;
}
