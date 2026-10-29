// documents.js — turn imported/dropped files into plain text and sections for note extraction.
// Text-like files are read directly; PDFs go through pdf.js (vendored in ./vendor, works offline).

export const ACCEPT = '.txt,.md,.markdown,.text,.csv,.json,.log,.rst,.html,.htm,.xml,.yaml,.yml,.toml,.pdf,text/plain,application/pdf';
const TEXT_EXT = /\.(txt|md|markdown|text|csv|json|log|rst|html?|xml|yaml|yml|toml|ini|cfg|tex|org)$/i;

/** Parse a File into { name, text, pages }. onProgress({page, pages}) fires per PDF page. */
export async function parseFile(file, onProgress) {
  const name = file.name;
  if (/\.pdf$/i.test(name) || file.type === 'application/pdf') return parsePdf(file, onProgress);
  if (TEXT_EXT.test(name) || file.type.startsWith('text/') || file.type === 'application/json' || !file.type) {
    let text = await file.text();
    if (/\.html?$/i.test(name) || file.type === 'text/html') text = htmlToText(text);
    return { name, text: normalize(text), pages: null };
  }
  throw new Error(`Unsupported file type: ${name}`);
}

function htmlToText(html) {
  const doc = new DOMParser().parseFromString(html, 'text/html');
  for (const el of doc.querySelectorAll('script, style, noscript, template')) el.remove();
  return doc.body?.innerText || doc.body?.textContent || '';
}

function normalize(t) {
  return String(t || '').replace(/\r\n?/g, '\n').replace(/ /g, ' ').replace(/[ \t]+\n/g, '\n').replace(/\n{3,}/g, '\n\n').trim();
}

let pdfjsPromise = null;
function loadPdfjs() {
  if (!pdfjsPromise) {
    pdfjsPromise = import('./vendor/pdf.min.mjs').then((lib) => {
      lib.GlobalWorkerOptions.workerSrc = new URL('./vendor/pdf.worker.min.mjs', import.meta.url).href;
      return lib;
    });
  }
  return pdfjsPromise;
}

async function parsePdf(file, onProgress) {
  const pdfjs = await loadPdfjs();
  const data = new Uint8Array(await file.arrayBuffer());
  const doc = await pdfjs.getDocument({ data }).promise;
  const pages = [];
  for (let p = 1; p <= doc.numPages; p++) {
    onProgress?.({ page: p, pages: doc.numPages });
    const page = await doc.getPage(p);
    const content = await page.getTextContent();
    let text = '', lastX = null, lastY = null;
    for (const it of content.items) {
      if (typeof it.str !== 'string') continue;
      const x = it.transform?.[4] ?? 0, y = it.transform?.[5] ?? 0;
      if (lastY !== null && Math.abs(y - lastY) > 2) text += '\n';               // new line when the baseline moves
      else if (lastX !== null && x - lastX > 1.5 && text && !/\s$/.test(text) && !/^\s/.test(it.str)) text += ' '; // gap between words
      text += it.str;
      if (it.hasEOL) text += '\n';
      lastX = x + (it.width || 0); lastY = y;
    }
    pages.push(normalize(text));
    page.cleanup();
  }
  const numPages = doc.numPages;
  await doc.destroy();
  const text = pages.map((t, i) => (t ? `[Page ${i + 1}]\n${t}` : '')).filter(Boolean).join('\n\n');
  return { name: file.name, text, pages: numPages };
}

/** Split text into sections of roughly `size` characters on paragraph (then sentence) boundaries. */
export function chunkText(text, { size = 2400 } = {}) {
  const paras = String(text || '').split(/\n\s*\n/).map((s) => s.trim()).filter(Boolean);
  const chunks = [];
  let cur = '';
  const push = () => { if (cur.trim()) chunks.push(cur.trim()); cur = ''; };
  for (let p of paras) {
    while (p.length > size * 1.5) {
      let cut = p.lastIndexOf('. ', size);
      if (cut < size * 0.4) cut = p.lastIndexOf(' ', size);
      if (cut < size * 0.4) cut = size;
      push(); chunks.push(p.slice(0, cut + 1).trim()); p = p.slice(cut + 1).trim();
    }
    if (cur && (cur.length + p.length + 2) > size) push();
    cur = cur ? cur + '\n\n' + p : p;
  }
  push();
  return chunks;
}

/** A readable title from a file name: "my_report-v2.pdf" -> "my report v2". */
export function titleFromFilename(name) {
  return String(name || 'Document').replace(/\.[^.]+$/, '').replace(/[_\-]+/g, ' ').replace(/\s+/g, ' ').trim().slice(0, 80) || 'Document';
}
