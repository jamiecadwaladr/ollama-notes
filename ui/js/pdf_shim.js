// The one piece of the page left in JS: calling the vendored pdf.js (works offline). It returns each page's text
// runs; src/documents.rs turns them into lines. Paths are absolute because wasm-bindgen copies this file into a
// snippets folder.

let pdfjs = null;

export async function pdfPages(data, onProgress) {
  if (!pdfjs) {
    pdfjs = await import('/vendor/pdf.min.mjs');
    pdfjs.GlobalWorkerOptions.workerSrc = '/vendor/pdf.worker.min.mjs';
  }
  const doc = await pdfjs.getDocument({ data }).promise;
  const pages = [];
  for (let p = 1; p <= doc.numPages; p++) {
    onProgress(p, doc.numPages);
    const page = await doc.getPage(p);
    const content = await page.getTextContent();
    pages.push(content.items.filter((it) => typeof it.str === 'string').map((it) => ({
      s: it.str, x: it.transform?.[4] ?? 0, y: it.transform?.[5] ?? 0, w: it.width || 0, eol: !!it.hasEOL,
    })));
    page.cleanup();
  }
  await doc.destroy();
  return pages;
}
