// Small, safe Markdown renderer: escapes HTML first, then supports fenced code,
// inline code, bold/italic, headings, lists, blockquotes, rules and http links.

const esc = (s) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
const PH = ''; // private-use char used to stash fenced code blocks

function inline(s) {
  s = esc(s);
  s = s.replace(/`([^`]+)`/g, '<code>$1</code>');
  s = s.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
  s = s.replace(/(^|[^*\w])\*([^*\n]+)\*(?!\w)/g, '$1<em>$2</em>');
  s = s.replace(/(^|[^_\w])_([^_\n]+)_(?!\w)/g, '$1<em>$2</em>');
  s = s.replace(/\[([^\]]+)\]\((https?:\/\/[^\s)]+)\)/g, '<a href="$2" target="_blank" rel="noopener">$1</a>');
  return s;
}

export function renderMarkdown(src) {
  if (!src) return '';
  const blocks = [];
  src = src.replace(/```([\w+-]*)[^\n]*\n([\s\S]*?)(```|$)/g, (_, lang, code) => {
    blocks.push(`<pre><code class="lang-${esc(lang)}">${esc(code.replace(/\n$/, ''))}</code></pre>`);
    return `${PH}${blocks.length - 1}${PH}`;
  });
  const phLine = new RegExp(`^${PH}(\\d+)${PH}$`);
  const phAny = new RegExp(`${PH}(\\d+)${PH}`, 'g');
  const out = [];
  let para = [], list = null;
  const flushPara = () => { if (para.length) { out.push(`<p>${inline(para.join(' '))}</p>`); para = []; } };
  const flushList = () => { if (list) { out.push(`<${list.type}>${list.items.map((i) => `<li>${inline(i)}</li>`).join('')}</${list.type}>`); list = null; } };
  for (const raw of src.split('\n')) {
    const line = raw.replace(/\s+$/, '');
    let m;
    if ((m = line.trim().match(phLine))) { flushPara(); flushList(); out.push(blocks[+m[1]]); continue; }
    if (!line.trim()) { flushPara(); flushList(); continue; }
    if ((m = line.match(/^(#{1,6})\s+(.*)/))) { flushPara(); flushList(); const l = Math.min(m[1].length + 2, 6); out.push(`<h${l}>${inline(m[2])}</h${l}>`); continue; }
    if ((m = line.match(/^\s*[-*•]\s+(.*)/))) { flushPara(); if (!list || list.type !== 'ul') { flushList(); list = { type: 'ul', items: [] }; } list.items.push(m[1]); continue; }
    if ((m = line.match(/^\s*\d+[.)]\s+(.*)/))) { flushPara(); if (!list || list.type !== 'ol') { flushList(); list = { type: 'ol', items: [] }; } list.items.push(m[1]); continue; }
    if ((m = line.match(/^\s*>\s?(.*)/))) { flushPara(); flushList(); out.push(`<blockquote>${inline(m[1])}</blockquote>`); continue; }
    if (/^\s*([-*_]\s*){3,}$/.test(line)) { flushPara(); flushList(); out.push('<hr>'); continue; }
    if (list && /^\s{2,}\S/.test(raw)) { list.items[list.items.length - 1] += ' ' + line.trim(); continue; }
    flushList(); para.push(line.trim());
  }
  flushPara(); flushList();
  return out.join('\n').replace(phAny, (_, i) => blocks[+i]);
}
