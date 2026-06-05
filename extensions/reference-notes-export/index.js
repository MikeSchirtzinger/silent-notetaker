/**
 * reference-notes-export — the R7 acceptance reference extension.
 *
 * Runs inside the host's null-origin sandboxed iframe (no allow-same-origin). It
 * has NO access to the host page, no network grant, and only the meeting data it
 * declared in manifest.json: transcript text, the four note categories, and
 * meeting metadata, plus the `panel` + `notification` UI surfaces.
 *
 * What it does:
 *   - tracks the live notes the host pushes (notes.update), grouped by category;
 *   - renders a running summary panel (decisions / actions / key points /
 *     questions counts + the latest few of each);
 *   - on meeting.stop, pulls a full snapshot via export.request and renders the
 *     exported Markdown into the panel (and posts a toast).
 *
 * The host injects only the bootstrap that gives us `globalThis.silent`:
 *   silent.extensionId      — our manifest name
 *   silent.onHostMessage(fn)— register the host→extension handler
 *   silent.post(message)    — send an ExtensionMessage { type, payload } back
 *   silent.renderLocal(html)— render HTML into our OWN sandbox document
 *
 * Every host message is a HostMessage body: { type, payload }. The host has
 * already gated each one against our grant set, so we only ever receive data we
 * were granted.
 */

const state = {
  meetingTitle: '',
  meetingId: null,
  transcriptSegments: 0,
  notes: { decision: [], action: [], keypoint: [], question: [] },
  exportedMarkdown: '',
};

const CATEGORY_LABEL = {
  decision: 'Decisions',
  action: 'Action Items',
  keypoint: 'Key Points',
  question: 'Open Questions',
};

function esc(s) {
  return String(s == null ? '' : s)
    .replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

function renderPanel() {
  const counts = Object.keys(CATEGORY_LABEL)
    .map((c) => `${CATEGORY_LABEL[c]}: <b>${state.notes[c].length}</b>`)
    .join(' &middot; ');

  const sections = Object.keys(CATEGORY_LABEL).map((cat) => {
    const items = state.notes[cat];
    if (!items.length) return '';
    const lis = items.slice(-3).map((n) => `<li>${esc(n.text)}</li>`).join('');
    return `<div class="ext-sec"><div class="ext-sec-h">${CATEGORY_LABEL[cat]}</div><ul>${lis}</ul></div>`;
  }).join('');

  const exportBlock = state.exportedMarkdown
    ? `<div class="ext-sec"><div class="ext-sec-h">Exported Markdown</div>
         <pre class="ext-md">${esc(state.exportedMarkdown)}</pre></div>`
    : '';

  const html = `
    <style>
      .ext-title { font-weight:600; margin-bottom:4px; }
      .ext-meta { color:#6b6b80; font-size:11px; margin-bottom:10px; }
      .ext-counts { color:#00d4aa; font-size:12px; margin-bottom:12px; }
      .ext-sec { margin-bottom:12px; }
      .ext-sec-h { font-size:11px; text-transform:uppercase; letter-spacing:.5px;
        color:#7b68ee; margin-bottom:4px; }
      .ext-sec ul { margin:0; padding-left:16px; }
      .ext-sec li { margin-bottom:3px; }
      .ext-md { white-space:pre-wrap; background:#0a0a0f; border:1px solid #1e1e2e;
        border-radius:6px; padding:8px; font-size:11px; max-height:220px; overflow:auto; }
    </style>
    <div class="ext-title">Reference: Notes Export</div>
    <div class="ext-meta">${esc(state.meetingTitle || 'No active meeting')} &middot; ${state.transcriptSegments} transcript segment(s)</div>
    <div class="ext-counts">${counts}</div>
    ${sections || '<div class="ext-meta">Notes will appear here as the meeting builds…</div>'}
    ${exportBlock}
  `;

  // Render into OUR OWN sandbox document AND ask the host to keep the canonical
  // panel content in sync (render.panel echoes it back into this same iframe;
  // this also exercises the render.panel UI capability for the R7 witness).
  silent.renderLocal(html);
  silent.post({ type: 'render.panel', payload: { html } });
}

/**
 * Build the exported Markdown from an export.response snapshot. The snapshot
 * carries only the surfaces we were granted: { transcript, notes, speakers }.
 */
function buildMarkdown(snapshot) {
  const lines = [];
  lines.push(`# ${state.meetingTitle || 'Meeting Notes'}`);
  lines.push('');

  const notes = (snapshot && snapshot.notes) || [];
  const byCat = { decision: [], action: [], keypoint: [], question: [] };
  for (const n of notes) {
    if (byCat[n.category]) byCat[n.category].push(n);
  }
  for (const cat of ['decision', 'action', 'keypoint', 'question']) {
    if (!byCat[cat].length) continue;
    lines.push(`## ${CATEGORY_LABEL[cat]}`);
    for (const n of byCat[cat]) lines.push(`- ${n.text}`);
    lines.push('');
  }

  const segs = (snapshot && snapshot.transcript) || [];
  if (segs.length) {
    lines.push(`## Transcript (${segs.length} segment${segs.length === 1 ? '' : 's'})`);
    for (const s of segs) lines.push(`- ${s.text}`);
    lines.push('');
  }

  return lines.join('\n').trim() + '\n';
}

silent.onHostMessage((msg) => {
  if (!msg || typeof msg.type !== 'string') return;
  const p = msg.payload || {};
  switch (msg.type) {
    case 'meeting.start':
      state.meetingTitle = p.title || '';
      state.meetingId = p.meetingId || null;
      state.notes = { decision: [], action: [], keypoint: [], question: [] };
      state.transcriptSegments = 0;
      state.exportedMarkdown = '';
      renderPanel();
      break;

    case 'transcript.update':
      state.transcriptSegments += 1;
      renderPanel();
      break;

    case 'notes.update':
      if (state.notes[p.category]) {
        state.notes[p.category].push({ text: p.text, speaker: p.speaker || null });
        renderPanel();
      }
      break;

    case 'meeting.stop':
      // Pull a full snapshot of everything we are entitled to, then export.
      silent.post({
        type: 'export.request',
        payload: {
          include: [
            'transcript.text',
            'notes.decisions', 'notes.actions', 'notes.keypoints', 'notes.questions',
          ],
        },
      });
      break;

    case 'export.response':
      state.exportedMarkdown = buildMarkdown(p);
      // renderPanel() re-renders (now including the export block) AND re-posts
      // render.panel, so the host's panel reflects the exported Markdown.
      renderPanel();
      silent.post({
        type: 'render.notification',
        payload: { text: 'Notes exported to Markdown (' + state.exportedMarkdown.split('\n').length + ' lines).' },
      });
      break;

    default:
      // Unknown/future message type — ignore.
      break;
  }
});

// Initial render so the panel is never blank.
renderPanel();
