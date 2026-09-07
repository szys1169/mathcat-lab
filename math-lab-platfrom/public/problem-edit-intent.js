// Only direct, complete replacement instructions are edits. Questions and
// suggestions remain discussion; the whiteboard editor handles other phrasing.
export function explicitProblemEdit(text) {
  const match=String(text||'').trim().match(/^(?:请)?(?:把|将)?(?:当前)?(数学问题|数学题面|研究说明|研究描述)(?:修改为|改为|替换为|更新为)\s*[:：]\s*([\s\S]+)$/u);
  if(!match)return null;
  const field=/^数学/.test(match[1])?'math_statement':'research_description';
  return {[field]:match[2].trim()};
}
