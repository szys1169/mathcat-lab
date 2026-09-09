// Saved prose often omits math delimiters. Read balanced script groups rather
// than truncating them with a regular expression. Code is protected upstream.
const names='Ann|dim|Mat|Hom|End|Ker|ker|coker|Im|rank|Tr|Tor|Ext|sin|cos|tan|log|exp|alpha|beta|gamma|delta|epsilon|theta|lambda|sigma|eta|chi|Omega|Phi|CP';
const atom=new RegExp('(?:\\\\(?:mathfrak|mathbb|mathcal|mathbf|mathrm)\\{[^{}]+\\}|\\\\[A-Za-z]+|'+names+'|[A-Za-zα-ωΑ-Ω∂∇∏⊗Σ𝔪𝔫𝔭])', 'uy');
const greek=/\b(alpha|beta|gamma|delta|epsilon|theta|lambda|sigma|eta|chi|Omega|Phi)\b/g;
function group(text,start){
  const pairs={'{':'}','(':')','[':']'},stack=[pairs[text[start]]];
  if(!stack[0])return null;
  for(let i=start+1;i<text.length;i++){
    if(/[\r\n]/.test(text[i]))return null;
    if(text[i]==='\\'){i++;continue;}
    if(pairs[text[i]])stack.push(pairs[text[i]]);
    else if(/[})\]]/.test(text[i])){
      if(text[i]!==stack.pop())return null;
      if(!stack.length)return {body:text.slice(start+1,i),end:i+1};
    }
  }
  return null;
}
function normalize(value){
  return value.replace(/\\_/g,'_').replace(greek,'\\$1')
    .replace(/𝔪/g,'\\mathfrak{m}').replace(/𝔫/g,'\\mathfrak{n}').replace(/𝔭/g,'\\mathfrak{p}')
    .replace(/∂/g,'\\partial ').replace(/∇/g,'\\nabla ').replace(/Σ/g,'\\sum ').replace(/∏/g,'\\prod ').replace(/⊗/g,'\\otimes ')
    .replace(/−/g,'-').replace(/≥/g,'\\ge ').replace(/≤/g,'\\le ');
}
function script(text,start){
  const grouped=group(text,start);if(grouped)return {...grouped,grouped:true};
  if(text[start]==='*')return {body:'*',end:start+1};
  const number=text.slice(start).match(/^\d+/);if(number)return {body:number[0],end:start+number[0].length};
  atom.lastIndex=start;const match=atom.exec(text);return match?{body:match[0],end:atom.lastIndex}:null;
}
export function extractBareMath(text,replace){
  let out='',cursor=0;
  for(let i=0;i<text.length;i++){
    const before=text[i-1]||'',prefix=text.slice(0,i).split(/\s/).at(-1);
    if(/[A-Za-z_\\.]/.test(before)&&!/[∂∇∏⊗Σ]/.test(text[i]))continue;
    if(/(?:^|[A-Za-z]:)[/\\][^\s]*$/.test(prefix)||/https?:\/\/\S*$/.test(prefix))continue;
    atom.lastIndex=i;const match=atom.exec(text);let base=match?.[0],end=atom.lastIndex;
    if(!base&&/[([]/.test(text[i])){const g=group(text,i);if(g&&/[A-Za-z0-9α-ω]/.test(g.body)){base=text.slice(i,g.end);end=g.end;}}
    if(!base)continue;
    while(text[end]==="'"){base+="'";end++;}
    const scripts=[];let invalid=false;
    while(text[end]==='_'||text[end]==='^'||text.slice(end,end+2)==='\\_'){
      const sign=text[end]==='^'?'^':'_',width=text[end]==='\\'?2:1;
      const item=script(text,end+width);if(!item){invalid=true;break;}
      // x_i^a_i conventionally means x_i^{a_i}; keep the exponent's index.
      if(scripts.some(s=>s.sign===sign)){
        const last=scripts.at(-1);
        if(sign==='_'&&last?.sign==='^'&&!last.grouped){last.body+='_{'+item.body+'}';end=item.end;continue;}
        invalid=true;break;
      }
      scripts.push({...item,sign});end=item.end;
    }
    if(!scripts.length||invalid)continue;
    // An unbraced multi-letter suffix is an identifier, not an inferred index.
    if(/[A-Za-z_]/.test(text[end]||'')&&!scripts.at(-1).grouped&&/^[A-Za-z_]{3,}/.test(text.slice(end)))continue;
    if(text[end]==='.'&&/[A-Za-z]/.test(text[end+1]||''))continue;
    let source=normalize(base);
    if(/^(Ann|dim|Mat|Hom|End|Ker|ker|coker|Im|rank|Tr|Tor|Ext)$/.test(base))source='\\operatorname{'+base+'}';
    if(/^(sin|cos|tan|log|exp)$/.test(base))source='\\'+base;
    source+=scripts.map(s=>s.sign+'{'+normalize(s.body)+'}').join('');
    out+=text.slice(cursor,i)+replace(text.slice(i,end),source);cursor=end;i=end-1;
  }
  return out+text.slice(cursor);
}
