function escapeHtml(value) {
  return String(value)
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

function renderInline(value) {
  let html = escapeHtml(value);
  html = html.replace(/`([^`]+)`/g, '<code>$1</code>');
  html = html.replace(/\[([^\]]+)\]\(([^)\s]+)\)/g, (match, label, href) => {
    const url = href.replace(/&amp;/g, '&');
    if (!/^https?:\/\//i.test(url) && !/^mailto:/i.test(url)) return match;
    return `<a href="${escapeHtml(url)}" target="_blank" rel="noopener noreferrer">${label}</a>`;
  });
  html = html.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
  html = html.replace(/__([^_]+)__/g, '<strong>$1</strong>');
  html = html.replace(/(^|[\s(])\*([^*\n]+)\*/g, '$1<em>$2</em>');
  html = html.replace(/(^|[\s(])_([^_\n]+)_/g, '$1<em>$2</em>');
  return html;
}

function splitTableRow(line) {
  let text = line.trim();
  if (text.startsWith('|')) text = text.slice(1);
  if (text.endsWith('|')) text = text.slice(0, -1);
  return text.split('|').map(cell => cell.trim());
}

function isTableRow(line) {
  const text = line.trim();
  return text.includes('|') && splitTableRow(line).length >= 2;
}

function isTableSeparator(line) {
  const cells = splitTableRow(line);
  return cells.length >= 2 && cells.every(cell => /^:?-+:?$/.test(cell));
}

function isTableStart(lines, index) {
  return isTableRow(lines[index]) && index + 1 < lines.length && isTableSeparator(lines[index + 1]);
}

function columnAlign(separator) {
  if (separator.startsWith(':') && separator.endsWith(':')) return 'center';
  if (separator.endsWith(':')) return 'right';
  return 'left';
}

function readTable(lines, index) {
  const header = splitTableRow(lines[index]);
  const aligns = splitTableRow(lines[index + 1]).map(columnAlign);
  const rows = [];
  let next = index + 2;
  while (next < lines.length && lines[next].trim() && isTableRow(lines[next]) && !isTableSeparator(lines[next])) {
    rows.push(splitTableRow(lines[next]));
    next += 1;
  }
  const head = header.map((cell, column) => `<th style="text-align:${aligns[column] || 'left'}">${renderInline(cell)}</th>`).join('');
  const body = rows.map(row => {
    const cells = header.map((_, column) => `<td style="text-align:${aligns[column] || 'left'}">${renderInline(row[column] || '')}</td>`).join('');
    return `<tr>${cells}</tr>`;
  }).join('');
  return {
    html: `<div class="agent-table-wrap"><table><thead><tr>${head}</tr></thead><tbody>${body}</tbody></table></div>`,
    next,
  };
}

function renderBlocks(source) {
  const lines = source.split('\n');
  const html = [];
  const paragraph = [];
  let index = 0;

  const flushParagraph = () => {
    if (paragraph.length === 0) return;
    html.push(`<p>${paragraph.map(renderInline).join('<br>')}</p>`);
    paragraph.length = 0;
  };

  while (index < lines.length) {
    const line = lines[index];
    if (!line.trim()) {
      flushParagraph();
      index += 1;
      continue;
    }
    if (isTableStart(lines, index)) {
      flushParagraph();
      const table = readTable(lines, index);
      html.push(table.html);
      index = table.next;
      continue;
    }
    const heading = /^(#{1,3})\s+(.+)$/.exec(line);
    if (heading) {
      flushParagraph();
      const level = heading[1].length;
      html.push(`<h${level}>${renderInline(heading[2])}</h${level}>`);
      index += 1;
      continue;
    }
    if (/^>\s?/.test(line)) {
      flushParagraph();
      const quote = [];
      while (index < lines.length && /^>\s?/.test(lines[index])) {
        quote.push(lines[index].replace(/^>\s?/, ''));
        index += 1;
      }
      html.push(`<blockquote>${quote.map(renderInline).join('<br>')}</blockquote>`);
      continue;
    }
    if (/^[-*]\s+/.test(line)) {
      flushParagraph();
      const items = [];
      while (index < lines.length && /^[-*]\s+/.test(lines[index])) {
        items.push(`<li>${renderInline(lines[index].replace(/^[-*]\s+/, ''))}</li>`);
        index += 1;
      }
      html.push(`<ul>${items.join('')}</ul>`);
      continue;
    }
    if (/^\d+\.\s+/.test(line)) {
      flushParagraph();
      const items = [];
      while (index < lines.length && /^\d+\.\s+/.test(lines[index])) {
        items.push(`<li>${renderInline(lines[index].replace(/^\d+\.\s+/, ''))}</li>`);
        index += 1;
      }
      html.push(`<ol>${items.join('')}</ol>`);
      continue;
    }
    paragraph.push(line);
    index += 1;
  }
  flushParagraph();
  return html.join('');
}

export function renderMarkdown(source) {
  const text = String(source ?? '').replace(/\r\n/g, '\n');
  const html = [];
  const pattern = /```[^\n`]*\n([\s\S]*?)```/g;
  let last = 0;
  for (const match of text.matchAll(pattern)) {
    html.push(renderBlocks(text.slice(last, match.index)));
    html.push(`<pre><code>${escapeHtml(match[1].replace(/\n$/, ''))}</code></pre>`);
    last = match.index + match[0].length;
  }
  html.push(renderBlocks(text.slice(last)));
  return html.join('');
}
