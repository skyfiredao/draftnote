import { EditorView, keymap, drawSelection, Decoration, ViewPlugin, lineNumbers } from "@codemirror/view";
import { EditorState, StateField, StateEffect, RangeSetBuilder } from "@codemirror/state";
import { history, historyKeymap, defaultKeymap } from "@codemirror/commands";
import { markdown } from "@codemirror/lang-markdown";
import { json } from "@codemirror/lang-json";
import {
  StreamLanguage,
  syntaxHighlighting,
  defaultHighlightStyle,
  HighlightStyle,
  foldGutter,
  codeFolding,
  foldKeymap,
  forceParsing,
  syntaxTree,
  foldEffect,
  foldInside,
} from "@codemirror/language";
import { tags as t } from "@lezer/highlight";
import { shell } from "@codemirror/legacy-modes/mode/shell";
import { marked } from "marked";
import DOMPurify from "dompurify";
import mermaid from "mermaid";
import Prism from "prismjs";
import "prismjs/components/prism-bash";
import "prismjs/components/prism-json";

const markedRenderer = new marked.Renderer();
const defaultCodeRenderer = markedRenderer.code.bind(markedRenderer);
markedRenderer.code = function (code, lang, escaped) {
  const language = (lang || "").split(/\s+/)[0];
  if (language === "mermaid") {
    const src = typeof code === "string" ? code : (code && code.text) || "";
    return '<pre class="mermaid">' + escapeHtml(src) + "</pre>";
  }
  return defaultCodeRenderer(code, lang, escaped);
};
marked.use({ renderer: markedRenderer });

function renderMarkdown(md) {
  const html = marked.parse(md || "");
  return DOMPurify.sanitize(html, {
    ADD_TAGS: ["pre"],
    ADD_ATTR: ["class"],
    FORBID_TAGS: ["style", "iframe", "object", "embed", "form"],
    FORBID_ATTR: ["style"],
  });
}

function renderCodeBlock(body, lang) {
  const grammar = Prism.languages[lang];
  const highlighted = grammar ? Prism.highlight(body || "", grammar, lang) : escapeHtml(body || "");
  const html = '<pre class="code-block language-' + lang + '"><code>' + highlighted + "</code></pre>";
  return DOMPurify.sanitize(html, {
    ADD_TAGS: ["pre", "code", "span"],
    ADD_ATTR: ["class"],
    FORBID_TAGS: ["style", "iframe", "object", "embed", "form"],
    FORBID_ATTR: ["style"],
  });
}

function renderNoteBody(body, filetype) {
  if (filetype === "sh") return renderCodeBlock(body, "bash");
  if (filetype === "json") return renderCodeBlock(body, "json");
  return renderMarkdown(body);
}

function escapeHtml(s) {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

function initMermaid() {
  const dark = document.documentElement.getAttribute("data-theme") === "dark";
  mermaid.initialize({
    startOnLoad: false,
    theme: dark ? "dark" : "default",
    securityLevel: "antiscript",
    fontFamily:
      '-apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif',
  });
}

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  SAVED_PIN_MASK,
  configureArguments,
  canSaveSettings,
  decryptionErrorMessage,
  encryptionPinFingerprint,
  encryptionPinSubmission,
  isEncryptionPinValid,
  runSettingsValidation,
  settingsActions,
  settingsMayClose,
  settingsStageAfterValidation,
  shouldKeepSavedPin,
} from "./settings-pin.js";

const go = {
  GetConfig: () => invoke("get_config"),
  BeginSettingsValidation: () => invoke("begin_settings_validation"),
  SettingsClosed: () => invoke("settings_closed"),
  Configure: (repoUrl, serverUrl, username, token, useApi, syncIntervalSeconds, keepToken, encryptionPin) =>
    invoke("configure", {
      repoUrl,
      serverUrl,
      username,
      token,
      useApi,
      syncIntervalSeconds,
      keepToken,
      encryptionPin,
    }),
  ListNotes: () => invoke("list_notes"),
  AllTags: () => invoke("all_tags"),
  NotesByTag: (tag) => invoke("notes_by_tag", { tag }),
  SearchNotes: (q) => invoke("search_notes", { q }),
  LoadNote: (id) => invoke("load_note", { id }),
  CreateNote: (title, tags, body) => invoke("create_note", { title, tags, body }),
  UpdateNote: (id, title, tags, body) => invoke("update_note", { id, title, tags, body }),
  DeleteNote: (id) => invoke("delete_note", { id }),
  DuplicateNote: (id) => invoke("duplicate_note", { id }),
  SetPinned: (id, pinned) => invoke("set_pinned", { id, pinned }),
  SetNoteFileType: (id, ft) => invoke("set_note_file_type", { id, ft }),
  SetTrashed: (id, trashed) => invoke("set_trashed", { id, trashed }),
  PurgeExpiredTrash: () => invoke("purge_expired_trash"),
  PushNote: (id) => invoke("push_note", { id }),
  PushDelete: (id) => invoke("push_delete", { id }),
  RevisionList: (id, page, perPage) => invoke("revision_list", { id, page, perPage }),
  RevisionDiff: (id, sha) => invoke("revision_diff", { id, sha }),
  RevisionBody: (id, sha) => invoke("revision_body", { id, sha }),
  RestoreRevision: (id, sha) => invoke("restore_revision", { id, sha }),
  ValidateSync: () => invoke("validate_sync"),
  ImportFromDir: () => invoke("import_from_dir"),
  ImportFromFiles: () => invoke("import_from_files"),
  ExportView: (view, format) => invoke("export_view", { view, format }),
};

let currentID = null;
let editor = null;
let allNotes = [];
let activeView = "all";
let activeTag = null;
let searchTerm = "";
let saveTimer = null;
let pushTimer = null;
let findTerm = "";
let ctxTargetID = null;
let selectedIDs = new Set();
let anchorID = null;
let currentTags = [];

function firstLineTitle(body) {
  const firstLine = (body || "").split("\n").find((l) => l.trim().length > 0);
  return (firstLine || "").replace(/^#+\s*/, "").trim();
}

const titleMark = Decoration.line({ class: "cm-md-title" });

const conflictMarkerRe = /^(<{7} local version|={7}|>{7} remote version)$/;
const conflictLineMark = Decoration.line({ class: "cm-conflict-marker" });
const conflictBodyMark = Decoration.line({ class: "cm-conflict-body" });

const conflictPlugin = ViewPlugin.fromClass(
  class {
    constructor(view) {
      this.decorations = this.build(view);
    }
    update(u) {
      if (u.docChanged || u.viewportChanged) this.decorations = this.build(u.view);
    }
    build(view) {
      const builder = new RangeSetBuilder();
      const doc = view.state.doc;
      let inConflict = false;
      for (let ln = 1; ln <= doc.lines; ln++) {
        const line = doc.line(ln);
        const text = line.text;
        if (conflictMarkerRe.test(text)) {
          builder.add(line.from, line.from, conflictLineMark);
          if (text.startsWith("<")) inConflict = true;
          else if (text.startsWith(">")) inConflict = false;
        } else if (inConflict) {
          builder.add(line.from, line.from, conflictBodyMark);
        }
      }
      return builder.finish();
    }
  },
  { decorations: (v) => v.decorations }
);

const titlePlugin = ViewPlugin.fromClass(
  class {
    constructor(view) {
      this.decorations = this.build(view);
    }
    update(u) {
      if (u.docChanged || u.viewportChanged) this.decorations = this.build(u.view);
    }
    build(view) {
      const builder = new RangeSetBuilder();
      const first = view.state.doc.line(1);
      if (first.length > 0) builder.add(first.from, first.from, titleMark);
      return builder.finish();
    }
  },
  { decorations: (v) => v.decorations }
);

const setFind = StateEffect.define();

function buildFindDeco(text, term) {
  const builder = new RangeSetBuilder();
  if (!term) return builder.finish();
  const lower = text.toLowerCase();
  const q = term.toLowerCase();
  let i = 0;
  while ((i = lower.indexOf(q, i)) !== -1) {
    builder.add(i, i + q.length, Decoration.mark({ class: "cm-find-match" }));
    i += q.length;
  }
  return builder.finish();
}

const findField = StateField.define({
  create() {
    return Decoration.none;
  },
  update(deco, tr) {
    for (const e of tr.effects) {
      if (e.is(setFind)) {
        return buildFindDeco(tr.state.doc.toString(), e.value);
      }
    }
    if (tr.docChanged && findTerm) {
      return buildFindDeco(tr.state.doc.toString(), findTerm);
    }
    return deco.map(tr.changes);
  },
  provide: (f) => EditorView.decorations.from(f),
});

function languageExtension(filetype) {
  if (filetype === "sh") return StreamLanguage.define(shell);
  if (filetype === "json") return json();
  return markdown();
}

const codeHighlightStyle = HighlightStyle.define([
  { tag: t.comment, color: "var(--syntax-comment)", fontStyle: "italic" },
  { tag: t.lineComment, color: "var(--syntax-comment)", fontStyle: "italic" },
  { tag: t.blockComment, color: "var(--syntax-comment)", fontStyle: "italic" },
  { tag: t.docComment, color: "var(--syntax-comment)", fontStyle: "italic" },
  { tag: t.punctuation, color: "var(--syntax-punct)" },
  { tag: t.bracket, color: "var(--syntax-punct)" },
  { tag: t.paren, color: "var(--syntax-punct)" },
  { tag: t.brace, color: "var(--syntax-punct)" },
  { tag: t.squareBracket, color: "var(--syntax-punct)" },
  { tag: t.separator, color: "var(--syntax-punct)" },
  { tag: t.keyword, color: "var(--syntax-key)", fontWeight: "600" },
  { tag: t.controlKeyword, color: "var(--syntax-key)", fontWeight: "600" },
  { tag: t.moduleKeyword, color: "var(--syntax-key)", fontWeight: "600" },
  { tag: t.operatorKeyword, color: "var(--syntax-key)", fontWeight: "600" },
  { tag: t.definitionKeyword, color: "var(--syntax-key)", fontWeight: "600" },
  { tag: t.string, color: "var(--syntax-string)" },
  { tag: t.special(t.string), color: "var(--syntax-string)" },
  { tag: t.escape, color: "var(--syntax-string)" },
  { tag: t.character, color: "var(--syntax-string)" },
  { tag: t.number, color: "var(--syntax-number)" },
  { tag: t.integer, color: "var(--syntax-number)" },
  { tag: t.float, color: "var(--syntax-number)" },
  { tag: t.bool, color: "var(--syntax-number)" },
  { tag: t.null, color: "var(--syntax-number)" },
  { tag: t.propertyName, color: "var(--syntax-prop)" },
  { tag: t.definition(t.propertyName), color: "var(--syntax-prop)" },
  { tag: t.attributeName, color: "var(--syntax-prop)" },
  { tag: t.variableName, color: "var(--syntax-var)" },
  { tag: t.definition(t.variableName), color: "var(--syntax-var)" },
  { tag: t.function(t.variableName), color: "var(--syntax-func)" },
  { tag: t.function(t.propertyName), color: "var(--syntax-func)" },
  { tag: t.operator, color: "var(--syntax-op)" },
  { tag: t.compareOperator, color: "var(--syntax-op)" },
  { tag: t.logicOperator, color: "var(--syntax-op)" },
  { tag: t.arithmeticOperator, color: "var(--syntax-op)" },
  { tag: t.bitwiseOperator, color: "var(--syntax-op)" },
  { tag: t.regexp, color: "var(--syntax-var)" },
]);

function initEditor(doc, readOnly, filetype) {
  const parent = document.getElementById("editor");
  parent.innerHTML = "";
  const isCode = filetype === "sh" || filetype === "json";
  const extensions = [
    lineNumbers(),
    history(),
    drawSelection({ cursorBlinkRate: 1060 }),
    keymap.of([...defaultKeymap, ...historyKeymap]),
    languageExtension(filetype),
  ];
  if (isCode) {
    extensions.push(syntaxHighlighting(codeHighlightStyle));
  }
  extensions.push(
    syntaxHighlighting(defaultHighlightStyle, { fallback: true }),
    EditorView.lineWrapping,
    conflictPlugin,
    findField,
    EditorView.updateListener.of((v) => {
      if (v.docChanged) {
        scheduleSave();
        updateFindScrollbar();
      }
    }),
  );
  if (!filetype || filetype === "md") {
    extensions.push(titlePlugin);
  }
  if (filetype === "json") {
    extensions.push(codeFolding(), foldGutter(), keymap.of(foldKeymap));
  }
  if (readOnly) {
    extensions.push(EditorState.readOnly.of(true));
    extensions.push(EditorView.editable.of(false));
  }
  editor = new EditorView({
    state: EditorState.create({
      doc: doc || "",
      extensions,
    }),
    parent,
  });
  if (filetype === "json") {
    forceParsing(editor, editor.state.doc.length, 2000);
    foldNestedJSON(editor);
  }
}

function foldNestedJSON(view) {
  const state = view.state;
  const tree = syntaxTree(state);
  const effects = [];
  let depth = 0;
  tree.iterate({
    enter: (nodeRef) => {
      if (nodeRef.name === "Object" || nodeRef.name === "Array") {
        depth++;
        if (depth > 1) {
          const range = foldInside(nodeRef.node);
          if (range && range.from < range.to) {
            effects.push(foldEffect.of(range));
          }
        }
      }
    },
    leave: (nodeRef) => {
      if (nodeRef.name === "Object" || nodeRef.name === "Array") depth--;
    },
  });
  if (effects.length) view.dispatch({ effects });
}

function editorText() {
  return editor ? editor.state.doc.toString() : "";
}

function scheduleSave() {
  if (saveTimer) clearTimeout(saveTimer);
  saveTimer = setTimeout(saveNote, 500);
  schedulePush();
}

function schedulePush() {
  const id = currentID;
  if (pushTimer) clearTimeout(pushTimer);
  pushTimer = setTimeout(() => {
    pushTimer = null;
    pushNote(id);
  }, 3000);
}

async function pushNote(id) {
  if (!go || !id) return;
  let result;
  try {
    result = await go.PushNote(id);
  } catch (e) {
  } finally {
    await loadAll();
  }
  if (result && result.Conflicted && id === currentID) {
    if (saveTimer) { clearTimeout(saveTimer); saveTimer = null; }
    if (pushTimer) { clearTimeout(pushTimer); pushTimer = null; }
    const n = await go.LoadNote(id);
    setTags(n.tags || []);
    initEditor(n.body, !!n.trashed, n.filetype);
  }
}

async function flushPending() {
  if (saveTimer) {
    clearTimeout(saveTimer);
    saveTimer = null;
    await saveNote();
  }
  if (pushTimer) {
    clearTimeout(pushTimer);
    pushTimer = null;
    await pushNote(currentID);
  }
}

function applyFind() {
  if (!editor) return;
  editor.dispatch({ effects: setFind.of(findTerm) });
  updateFindScrollbar();
}

function updateFindScrollbar() {
  const bar = document.getElementById("find-scrollbar");
  bar.innerHTML = "";
  const count = document.getElementById("find-count");
  if (!editor || !findTerm) {
    count.textContent = "";
    return;
  }
  const text = editor.state.doc.toString();
  const lower = text.toLowerCase();
  const q = findTerm.toLowerCase();
  const positions = [];
  let i = 0;
  while ((i = lower.indexOf(q, i)) !== -1) {
    positions.push(i);
    i += q.length;
  }
  count.textContent = positions.length ? positions.length + " found" : "0 found";
  const barHeight = bar.clientHeight || 1;
  const scroller = editor.scrollDOM;
  const contentHeight = scroller.scrollHeight || 1;
  positions.forEach((pos) => {
    let ratio;
    try {
      const block = editor.lineBlockAt(pos);
      ratio = (block.top + block.height / 2) / contentHeight;
    } catch (e) {
      ratio = pos / (text.length || 1);
    }
    if (ratio < 0) ratio = 0;
    if (ratio > 1) ratio = 1;
    const tick = document.createElement("div");
    tick.className = "find-tick";
    tick.style.top = ratio * barHeight + "px";
    tick.addEventListener("click", () => {
      editor.dispatch({ selection: { anchor: pos, head: pos + q.length }, scrollIntoView: true });
    });
    bar.appendChild(tick);
  });
}

function parseTags() {
  return currentTags.slice();
}

function renderTagChips() {
  const container = document.getElementById("tag-chips");
  const input = document.getElementById("tag-input");
  container.querySelectorAll(".tag-pill").forEach((el) => el.remove());
  currentTags.forEach((t, idx) => {
    const pill = document.createElement("span");
    pill.className = "tag-pill";
    pill.textContent = t;
    const x = document.createElement("button");
    x.className = "tag-x";
    x.textContent = "\u00d7";
    x.title = "Remove";
    x.addEventListener("click", (e) => {
      e.preventDefault();
      currentTags.splice(idx, 1);
      renderTagChips();
      scheduleSave();
    });
    pill.appendChild(x);
    container.insertBefore(pill, input);
  });
}

function commitTagInput() {
  const input = document.getElementById("tag-input");
  const raw = input.value.trim().replace(/,+$/, "").trim();
  if (!raw) {
    input.value = "";
    return false;
  }
  if (!currentTags.includes(raw)) {
    currentTags.push(raw);
  }
  input.value = "";
  renderTagChips();
  return true;
}

function setTags(tags) {
  currentTags = (tags || []).slice();
  const input = document.getElementById("tag-input");
  if (input) input.value = "";
  renderTagChips();
}

async function loadAll() {
  if (!go) return;
  if (searchTerm) {
    allNotes = (await go.SearchNotes(searchTerm)) || [];
  } else {
    allNotes = (await go.ListNotes()) || [];
  }
  renderNav();
  renderList();
}

function renderNav() {
  const tags = new Set();
  allNotes.forEach((n) => {
    if (n.trashed) return;
    (n.tags || []).forEach((t) => tags.add(t));
  });
  const container = document.getElementById("nav-tags");
  container.innerHTML = "";
  [...tags].sort().forEach((t) => {
    const d = document.createElement("div");
    d.className = "nav-item" + (activeView === "tag" && activeTag === t ? " active" : "");
    d.dataset.view = "tag";
    d.dataset.tag = t;
    d.textContent = t;
    d.addEventListener("click", () => {
      selectTag(t);
      document.getElementById("nav").focus();
    });
    d.addEventListener("contextmenu", (e) => {
      e.preventDefault();
      openExportMenu(e.clientX, e.clientY, "tag:" + t);
    });
    container.appendChild(d);
  });
  document.querySelectorAll("#nav > .nav-item[data-view]").forEach((el) => {
    el.classList.toggle("active", activeView === el.dataset.view);
  });
}

function selectView(view) {
  activeView = view;
  activeTag = null;
  selectedIDs.clear();
  anchorID = null;
  renderNav();
  renderList();
  setScreen("list");
}

function selectTag(tag) {
  activeView = "tag";
  activeTag = tag;
  selectedIDs.clear();
  anchorID = null;
  renderNav();
  renderList();
  setScreen("list");
}

function visibleNotes() {
  let notes = allNotes.slice();
  if (activeView === "trash") {
    notes = notes.filter((n) => n.trashed);
  } else {
    notes = notes.filter((n) => !n.trashed);
    if (activeView === "untagged") {
      notes = notes.filter((n) => !n.tags || n.tags.length === 0);
    } else if (activeView === "tag" && activeTag) {
      notes = notes.filter((n) => (n.tags || []).includes(activeTag));
    }
  }
  return notes;
}

function renderList() {
  const list = document.getElementById("note-list");
  list.innerHTML = "";
  const visible = visibleNotes();
  visible.forEach((n) => {
    const li = document.createElement("li");
    li.className = "note-item";
    li.dataset.id = n.id;
    if (n.id === currentID) li.classList.add("selected");
    if (selectedIDs.has(n.id)) li.classList.add("multi-selected");
    if (n.pinned) li.classList.add("pinned");
    if (n.has_conflict) li.classList.add("has-conflict");
    if (n.sync_failed) li.classList.add("sync-failed");
    if (n.filetype === "sh") li.classList.add("filetype-sh");
    if (n.filetype === "json") li.classList.add("filetype-json");

    const title = document.createElement("div");
    title.className = "note-title";
    title.textContent = n.title || "New note";

    li.appendChild(title);

    if (activeView === "trash" && n.trashed_at) {
      const expiry = document.createElement("div");
      expiry.className = "note-expiry";
      expiry.textContent = trashExpiryLabel(n.trashed_at);
      li.appendChild(expiry);
    }

    li.addEventListener("click", (e) => {
      if (e.shiftKey && anchorID) {
        rangeMultiSelect(visible, anchorID, n.id);
        renderList();
      } else if (e.ctrlKey || e.metaKey) {
        if (selectedIDs.has(n.id)) selectedIDs.delete(n.id);
        else selectedIDs.add(n.id);
        anchorID = n.id;
        renderList();
      } else {
        selectedIDs.clear();
        anchorID = n.id;
        openNote(n.id);
      }
      list.focus();
    });
    li.addEventListener("contextmenu", (e) => {
      e.preventDefault();
      if (!selectedIDs.has(n.id)) {
        selectedIDs.clear();
        anchorID = n.id;
        openNote(n.id);
      }
      openContextMenu(e.clientX, e.clientY, n);
    });
    list.appendChild(li);
  });
}

function rangeMultiSelect(visible, fromID, toID) {
  const fromIdx = visible.findIndex((n) => n.id === fromID);
  const toIdx = visible.findIndex((n) => n.id === toID);
  if (fromIdx < 0 || toIdx < 0) return;
  const [lo, hi] = fromIdx <= toIdx ? [fromIdx, toIdx] : [toIdx, fromIdx];
  selectedIDs.clear();
  for (let i = lo; i <= hi; i++) selectedIDs.add(visible[i].id);
}

function trashExpiryLabel(trashedAt) {
  const t = new Date(trashedAt).getTime();
  const expiresAt = t + 90 * 24 * 60 * 60 * 1000;
  const daysLeft = Math.max(0, Math.ceil((expiresAt - Date.now()) / (24 * 60 * 60 * 1000)));
  return daysLeft === 0
    ? "Expires today"
    : "Expires in " + daysLeft + " day" + (daysLeft === 1 ? "" : "s");
}

async function openNote(id) {
  if (id !== currentID) await flushPending();
  const n = await go.LoadNote(id);
  currentID = n.id;
  setTags(n.tags || []);
  const tagInput = document.getElementById("tag-input");
  tagInput.readOnly = !!n.trashed;
  initEditor(n.body, !!n.trashed, n.filetype);
  document.getElementById("editor-pane").classList.toggle("readonly", !!n.trashed);
  const previewEl = document.getElementById("preview-pane");
  if (previewEl && !previewEl.classList.contains("hidden")) {
    previewEl.innerHTML = renderNoteBody(n.body || "", n.filetype);
    if (!n.filetype || n.filetype === "md") runMermaid(previewEl);
  }
  renderList();
  setScreen("editor");
}

async function saveNote() {
  if (!go) return;
  if (currentID) {
    const cur = allNotes.find((x) => x.id === currentID);
    if (cur && cur.trashed) return;
  }
  const body = editorText();
  const tags = parseTags();
  const title = firstLineTitle(body);
  if (currentID) {
    await go.UpdateNote(currentID, title, tags, body);
  } else {
    if (body.trim().length === 0) return;
    const n = await go.CreateNote(title, tags, body);
    currentID = n.id;
  }
  await loadAll();
}

function resetEditorPane() {
  setTags([]);
  const tagInput = document.getElementById("tag-input");
  tagInput.readOnly = false;
  document.getElementById("editor-pane").classList.remove("readonly");
  document.getElementById("preview-pane").classList.add("hidden");
  document.getElementById("editor").classList.remove("hidden");
  document.getElementById("preview").classList.remove("active");
  initEditor("", false);
}

async function newNote() {
  await flushPending();
  currentID = null;
  resetEditorPane();
  renderList();
  editor.focus();
  setScreen("editor");
}

function toggleNav() {
  document.getElementById("nav").classList.toggle("hidden");
}

function setScreen(s) {
  document.body.dataset.screen = s;
  if (s !== "editor") document.body.classList.remove("tools-open");
}

function detectPlatform() {
  const ua = navigator.userAgent || "";
  const touch = navigator.maxTouchPoints || 0;
  const isIOS =
    /iPhone|iPad|iPod/.test(ua) || (touch > 1 && /Macintosh|Mac OS X/.test(ua));
  if (isIOS) document.body.classList.add("mobile");
}

function currentFileType() {
  const cur = allNotes.find((x) => x.id === currentID);
  return cur ? cur.filetype : "";
}

function toggleTheme() {
  const root = document.documentElement;
  const dark = root.getAttribute("data-theme") === "dark";
  root.setAttribute("data-theme", dark ? "light" : "dark");
  try {
    localStorage.setItem("theme", dark ? "light" : "dark");
  } catch (e) {}
  initMermaid();
  const previewEl = document.getElementById("preview-pane");
  if (previewEl && !previewEl.classList.contains("hidden")) {
    const ft = currentFileType();
    previewEl.innerHTML = renderNoteBody(editorText() || "", ft);
    if (!ft || ft === "md") runMermaid(previewEl);
  }
}

function togglePreview() {
  const editorEl = document.getElementById("editor");
  const previewEl = document.getElementById("preview-pane");
  const showingPreview = !previewEl.classList.contains("hidden");
  if (showingPreview) {
    previewEl.classList.add("hidden");
    editorEl.classList.remove("hidden");
    document.getElementById("preview").classList.remove("active");
    setViewportZoom(false);
  } else {
    const ft = currentFileType();
    previewEl.innerHTML = renderNoteBody(editorText() || "", ft);
    previewEl.classList.remove("hidden");
    editorEl.classList.add("hidden");
    document.getElementById("preview").classList.add("active");
    if (!ft || ft === "md") runMermaid(previewEl);
    setViewportZoom(true);
  }
}

function setViewportZoom(allow) {
  const m = document.querySelector('meta[name="viewport"]');
  if (!m) return;
  m.setAttribute(
    "content",
    allow
      ? "width=device-width, initial-scale=1.0, viewport-fit=cover"
      : "width=device-width, initial-scale=1.0, maximum-scale=1.0, user-scalable=no, viewport-fit=cover",
  );
}

function runMermaid(root) {
  const nodes = root.querySelectorAll("pre.mermaid");
  if (nodes.length === 0) return;
  nodes.forEach((n) => n.removeAttribute("data-processed"));
  mermaid.run({ nodes }).catch(() => {});
}

function initTheme() {
  let theme = "dark";
  try {
    theme = localStorage.getItem("theme") || "dark";
  } catch (e) {}
  document.documentElement.setAttribute("data-theme", theme);
}

let validatedFingerprint = null;
let settingsStage = "editing";

const settingFieldIds = ["cfg-repourl", "cfg-server-url", "cfg-username", "cfg-token", "cfg-useapi", "cfg-interval", "cfg-encryption-pin"];

function updateSettingsActions() {
  const actions = settingsActions(settingsStage);
  const validate = document.getElementById("cfg-validate");
  const cancel = document.getElementById("cfg-cancel");
  const save = document.getElementById("cfg-save");
  validate.disabled = !actions.validate;
  cancel.disabled = !actions.cancel;
  save.disabled = !actions.save || !canSaveSettings(validatedFingerprint, currentSettingsFingerprint());
  validate.classList.toggle("hidden", !actions.validate);
  cancel.classList.toggle("hidden", !actions.cancel);
  save.classList.toggle("hidden", !actions.save);
  for (const id of settingFieldIds) document.getElementById(id).disabled = !actions.editable;
}

function currentSettingsFingerprint() {
  const tokenInput = document.getElementById("cfg-token");
  const tokenPart = tokenInput.dataset.pristine === "1" ? "PRISTINE" : "NEW:" + tokenInput.value;
  return [
    document.getElementById("cfg-repourl").value.trim(),
    document.getElementById("cfg-server-url").value.trim(),
    document.getElementById("cfg-username").value.trim(),
    tokenPart,
    document.getElementById("cfg-useapi").checked ? "1" : "0",
    document.getElementById("cfg-interval").value,
    encryptionPinFingerprint(
      document.getElementById("cfg-encryption-pin").value,
      shouldKeepSavedPin(
        document.getElementById("cfg-encryption-pin").value,
        document.getElementById("cfg-encryption-pin").dataset.keepSavedPin === "1",
      ),
    ),
  ].join("|");
}

function updateSaveButtonState() {
  updateSettingsActions();
}

async function openSettings() {
  if (!go) return;
  const modal = document.getElementById("settings-modal");
  if (!modal.classList.contains("hidden")) return;
  modal.classList.remove("hidden");
  settingsStage = "validating";
  updateSettingsActions();
  resetValidateMsg();
  let cfg;
  try {
    await go.BeginSettingsValidation();
    await flushPending();
    cfg = (await go.GetConfig()) || {};
  } catch (error) {
    try {
      await go.SettingsClosed();
    } finally {
      modal.classList.add("hidden");
    }
    throw error;
  }
  document.getElementById("cfg-repourl").value = cfg.repo_url || "";
  document.getElementById("cfg-server-url").value = cfg.server_url || "";
  document.getElementById("cfg-username").value = cfg.username || "";
  document.getElementById("cfg-useapi").checked = cfg.use_api !== false;
  const tokenInput = document.getElementById("cfg-token");
  if (cfg.has_token) {
    tokenInput.value = "\u2022\u2022\u2022\u2022\u2022\u2022\u2022\u2022";
    tokenInput.dataset.pristine = "1";
  } else {
    tokenInput.value = "";
    tokenInput.dataset.pristine = "0";
  }
  tokenInput.oninput = () => {
    tokenInput.dataset.pristine = "0";
    validatedFingerprint = null;
    updateSaveButtonState();
  };
  document.getElementById("cfg-interval").value = cfg.sync_interval_seconds || 10;
  const pinInput = document.getElementById("cfg-encryption-pin");
  pinInput.value = cfg.encryption_enabled ? SAVED_PIN_MASK : "";
  pinInput.dataset.keepSavedPin = cfg.encryption_enabled ? "1" : "0";
  pinInput.onfocus = () => {
    if (pinInput.dataset.keepSavedPin === "1") pinInput.select();
  };
  pinInput.oninput = () => {
    pinInput.dataset.keepSavedPin = pinInput.value === SAVED_PIN_MASK ? "1" : "0";
    validatedFingerprint = null;
    updateSaveButtonState();
  };
  pinInput.placeholder = "6 digits to encrypt; blank for plaintext";
  ["cfg-repourl", "cfg-server-url", "cfg-username", "cfg-useapi", "cfg-interval", "cfg-encryption-pin"].forEach((id) => {
    const field = document.getElementById(id);
    field.oninput = field.onchange = () => {
      validatedFingerprint = null;
      updateSaveButtonState();
    };
  });
  document.getElementById("cfg-pin-lock-msg").textContent =
    cfg.encryption_locked_until > Math.floor(Date.now() / 1000)
      ? `PIN verification locked for ${cfg.encryption_locked_until - Math.floor(Date.now() / 1000)} seconds`
      : "";
  validatedFingerprint = null;
  settingsStage = "editing";
  updateSaveButtonState();
}

async function closeSettings(saved = false) {
  if (document.getElementById("settings-modal").classList.contains("hidden")) return;
  if (!settingsMayClose(settingsStage, saved)) return;
  try {
    await go.SettingsClosed();
  } finally {
    document.getElementById("settings-modal").classList.add("hidden");
    resetValidateMsg();
  }
}

function resetValidateMsg() {
  const msg = document.getElementById("cfg-validate-msg");
  msg.classList.add("hidden");
  msg.classList.remove("ok", "err");
  msg.textContent = "";
}

function showValidateMsg(text, kind) {
  const msg = document.getElementById("cfg-validate-msg");
  msg.textContent = text;
  msg.classList.remove("hidden", "ok", "err");
  if (kind) msg.classList.add(kind);
}

function emitSettingsSave() {
  void closeSettings(true);
}

function readSettingsForm() {
  const tokenInput = document.getElementById("cfg-token");
  const keepToken = tokenInput.dataset.pristine === "1";
  return {
    repoURL: document.getElementById("cfg-repourl").value.trim(),
    serverURL: document.getElementById("cfg-server-url").value.trim(),
    username: document.getElementById("cfg-username").value.trim(),
    useAPI: document.getElementById("cfg-useapi").checked,
    keepToken,
    token: keepToken ? "" : tokenInput.value.trim(),
    interval: parseInt(document.getElementById("cfg-interval").value, 10) || 10,
    encryptionPin: encryptionPinSubmission(
      document.getElementById("cfg-encryption-pin").value,
      shouldKeepSavedPin(
        document.getElementById("cfg-encryption-pin").value,
        document.getElementById("cfg-encryption-pin").dataset.keepSavedPin === "1",
      ),
    ),
  };
}

async function validateSettings() {
  if (!go || settingsStage !== "editing") return;
  const pinInput = document.getElementById("cfg-encryption-pin");
  const keepSavedPin = shouldKeepSavedPin(
    pinInput.value,
    pinInput.dataset.keepSavedPin === "1",
  );
  if (!isEncryptionPinValid(pinInput.value, keepSavedPin)) {
    showValidateMsg("PIN must be empty or exactly 6 digits", "err");
    return;
  }
  settingsStage = "validating";
  updateSettingsActions();
  validatedFingerprint = null;
  showValidateMsg("Validating…", "");
  try {
    const f = readSettingsForm();
    const validatedForm = currentSettingsFingerprint();
    const result = await runSettingsValidation({
      unlock: go.SettingsClosed,
      configure: () => go.Configure(...configureArguments(f)),
      validate: go.ValidateSync,
    });
    validatedFingerprint = null;
    if (validatedForm !== currentSettingsFingerprint()) {
      throw new Error("Settings changed during validation; validate again");
    }
    validatedFingerprint = validatedForm;
    settingsStage = settingsStageAfterValidation(true);
    showValidateMsg(result || "OK", "ok");
    updateSaveButtonState();
    void loadAll().catch((error) => console.error(error));
  } catch (e) {
    if (settingsStage !== "validated") {
      settingsStage = settingsStageAfterValidation(false);
      validatedFingerprint = null;
    }
    updateSaveButtonState();
    showValidateMsg(String(e && e.message ? e.message : e), "err");
  } finally {
    updateSettingsActions();
  }
}

async function saveSettings() {
  if (!go || settingsStage !== "validated" || !validatedFingerprint || validatedFingerprint !== currentSettingsFingerprint()) return;
  emitSettingsSave();
}

function getFontSize() {
  let v = 15;
  try {
    v = parseInt(localStorage.getItem("fontSize") || "15", 10);
  } catch (e) {}
  return isNaN(v) ? 15 : v;
}

function applyFontSize(size) {
  document.documentElement.style.setProperty("--editor-font-size", size + "px");
  try {
    localStorage.setItem("fontSize", String(size));
  } catch (e) {}
}

function changeFont(delta) {
  let size = getFontSize() + delta;
  size = Math.max(11, Math.min(28, size));
  applyFontSize(size);
}

function openContextMenu(x, y, n) {
  ctxTargetID = n.id;
  const menu = document.getElementById("ctx-menu");
  const isBatch = selectedIDs.has(n.id) && selectedIDs.size > 1;
  const count = selectedIDs.size;

  const pinItem = menu.querySelector('[data-action="pin"]');
  pinItem.textContent = n.pinned ? "Unpin" : "Pin to Top";

  ["pin", "duplicate", "history", "filetype-sh", "filetype-json", "filetype-md"].forEach((a) => {
    const el = menu.querySelector('[data-action="' + a + '"]');
    if (el) el.classList.toggle("hidden", isBatch);
  });

  const ft = n.filetype || "md";
  menu.querySelector('[data-action="filetype-sh"]').classList.toggle("hidden", isBatch || ft === "sh");
  menu.querySelector('[data-action="filetype-json"]').classList.toggle("hidden", isBatch || ft === "json");
  menu.querySelector('[data-action="filetype-md"]').classList.toggle("hidden", isBatch || ft === "md");

  const trashItem = menu.querySelector('[data-action="trash"]');
  trashItem.classList.toggle("hidden", !!n.trashed);
  trashItem.textContent = isBatch ? "Move " + count + " to Trash" : "Move to Trash";

  const restoreItem = menu.querySelector('[data-action="restore"]');
  restoreItem.classList.toggle("hidden", !n.trashed);
  restoreItem.textContent = isBatch ? "Restore " + count : "Restore from Trash";

  const delItem = menu.querySelector('[data-action="delete-permanent"]');
  delItem.classList.toggle("hidden", !n.trashed);
  delItem.textContent = isBatch ? "Delete " + count + " Permanently" : "Delete Permanently";

  menu.style.left = x + "px";
  menu.style.top = y + "px";
  menu.classList.remove("hidden");
}

function closeContextMenu() {
  document.getElementById("ctx-menu").classList.add("hidden");
  ctxTargetID = null;
}

let pendingExportView = null;

function openImportMenu() {
  const btn = document.getElementById("import-notes");
  const menu = document.getElementById("import-menu");
  const rect = btn.getBoundingClientRect();
  menu.classList.remove("hidden");
  menu.style.left = rect.left + "px";
  menu.style.top = rect.top - menu.offsetHeight - 4 + "px";
}

function closeImportMenu() {
  document.getElementById("import-menu").classList.add("hidden");
}

function openExportMenu(x, y, view) {
  pendingExportView = view;
  const menu = document.getElementById("export-menu");
  menu.style.left = x + "px";
  menu.style.top = y + "px";
  menu.classList.remove("hidden");
}

function closeExportMenu() {
  document.getElementById("export-menu").classList.add("hidden");
  pendingExportView = null;
}

async function importAction(action) {
  closeImportMenu();
  if (!go) return;
  try {
    let n = 0;
    if (action === "import-dir") n = await go.ImportFromDir();
    else if (action === "import-files") n = await go.ImportFromFiles();
    if (n > 0) await loadAll();
  } catch (e) {
    alert("Import failed: " + (e && e.message ? e.message : e));
  }
}

async function exportAction(action) {
  const view = pendingExportView;
  closeExportMenu();
  if (!go || !view) return;
  const format = action === "export-md" ? "md" : "txt";
  try {
    await go.ExportView(view, format);
  } catch (e) {
    alert("Export failed: " + (e && e.message ? e.message : e));
  }
}

async function contextAction(action) {
  const primaryID = ctxTargetID;
  closeContextMenu();
  if (!primaryID || !go) return;
  const batchIDs = selectedIDs.size > 1 && selectedIDs.has(primaryID)
    ? [...selectedIDs]
    : [primaryID];
  if (action === "history") {
    openRevisions(primaryID);
    return;
  }
  if (action === "pin") {
    const n = allNotes.find((x) => x.id === primaryID);
    await go.SetPinned(primaryID, !(n && n.pinned));
  } else if (action === "duplicate") {
    await go.DuplicateNote(primaryID);
  } else if (action === "filetype-sh" || action === "filetype-json" || action === "filetype-md") {
    const ft = action === "filetype-md" ? "" : action.slice("filetype-".length);
    await go.SetNoteFileType(primaryID, ft);
    await loadAll();
    if (primaryID === currentID) {
      const n = await go.LoadNote(primaryID);
      initEditor(n.body, !!n.trashed, n.filetype);
      const previewEl = document.getElementById("preview-pane");
      if (previewEl && !previewEl.classList.contains("hidden")) {
        previewEl.innerHTML = renderNoteBody(n.body || "", n.filetype);
        if (!n.filetype || n.filetype === "md") runMermaid(previewEl);
      }
    }
    return;
  } else if (action === "trash") {
    for (const id of batchIDs) {
      await go.SetTrashed(id, true);
    }
    if (batchIDs.includes(currentID)) {
      currentID = null;
      resetEditorPane();
    }
    selectedIDs.clear();
  } else if (action === "restore") {
    for (const id of batchIDs) {
      await go.SetTrashed(id, false);
    }
    selectedIDs.clear();
  } else if (action === "delete-permanent") {
    for (const id of batchIDs) {
      await go.DeleteNote(id);
      go.PushDelete(id).catch(() => {});
    }
    if (batchIDs.includes(currentID)) {
      currentID = null;
      resetEditorPane();
    }
    selectedIDs.clear();
  }
  await loadAll();
}

const REV_PER_PAGE = 30;
let revState = null;

async function openRevisions(id) {
  if (!go) return;
  revState = { id, page: 0, loading: false, done: false, selected: null, revs: [] };
  document.getElementById("rev-list").innerHTML = "";
  document.getElementById("rev-diff").innerHTML = "";
  const restore = document.getElementById("rev-restore");
  restore.disabled = true;
  document.getElementById("revision-modal").classList.remove("hidden");
  await loadRevisionPage();
  const listEl = document.getElementById("rev-list");
  if (listEl) listEl.focus();
}

function closeRevisions() {
  document.getElementById("revision-modal").classList.add("hidden");
  revState = null;
}

async function loadRevisionPage() {
  if (!revState || revState.loading || revState.done) return;
  revState.loading = true;
  const nextPage = revState.page + 1;
  try {
    const revs = await go.RevisionList(revState.id, nextPage, REV_PER_PAGE);
    revState.page = nextPage;
    if (!revs || revs.length < REV_PER_PAGE) revState.done = true;
    if (revs && revs.length > 0) appendRevisions(revs);
  } catch (e) {
    alert("History failed: " + (e && e.message ? e.message : e));
    revState.done = true;
  } finally {
    if (revState) revState.loading = false;
  }
}

function appendRevisions(revs) {
  const list = document.getElementById("rev-list");
  revs.forEach((r) => {
    revState.revs.push(r);
    const item = document.createElement("div");
    item.className = "rev-item";
    item.dataset.sha = r.sha;
    item.textContent = formatRevDate(r.date) + " " + r.sha.slice(0, 7);
    item.addEventListener("click", () => selectRevision(r.sha, item));
    list.appendChild(item);
  });
}

function formatRevDate(iso) {
  if (!iso) return "";
  const d = new Date(iso);
  if (isNaN(d.getTime())) return iso;
  return d.toLocaleString();
}

async function selectRevision(sha, item) {
  if (!revState) return;
  revState.selected = sha;
  document.querySelectorAll("#rev-list .rev-item").forEach((el) => {
    el.classList.toggle("selected", el === item);
  });
  document.getElementById("rev-restore").disabled = false;
  const diffEl = document.getElementById("rev-diff");
  diffEl.innerHTML = "";
  try {
    const idx = revState.revs.findIndex((r) => r.sha === sha);
    const prevSha = idx >= 0 && idx + 1 < revState.revs.length ? revState.revs[idx + 1].sha : "";
    const cur = await go.RevisionBody(revState.id, sha);
    const prev = prevSha ? await go.RevisionBody(revState.id, prevSha) : "";
    renderBodyDiff(diffEl, prev, cur);
  } catch (e) {
    diffEl.textContent = "Diff failed: " + (e && e.message ? e.message : e);
  }
}

function lineDiff(a, b) {
  const A = a === "" ? [] : a.split("\n");
  const B = b === "" ? [] : b.split("\n");
  const m = A.length, n = B.length;
  const lcs = Array.from({ length: m + 1 }, () => new Array(n + 1).fill(0));
  for (let i = m - 1; i >= 0; i--) {
    for (let j = n - 1; j >= 0; j--) {
      if (A[i] === B[j]) lcs[i][j] = lcs[i + 1][j + 1] + 1;
      else lcs[i][j] = Math.max(lcs[i + 1][j], lcs[i][j + 1]);
    }
  }
  const out = [];
  let i = 0, j = 0;
  while (i < m && j < n) {
    if (A[i] === B[j]) { out.push({ t: "ctx", s: A[i] }); i++; j++; }
    else if (lcs[i + 1][j] >= lcs[i][j + 1]) { out.push({ t: "del", s: A[i] }); i++; }
    else { out.push({ t: "add", s: B[j] }); j++; }
  }
  while (i < m) { out.push({ t: "del", s: A[i++] }); }
  while (j < n) { out.push({ t: "add", s: B[j++] }); }
  return out;
}

function renderBodyDiff(el, prev, cur) {
  const lines = lineDiff(prev, cur);
  if (lines.length === 0) {
    el.innerHTML = '<div class="rev-empty">No changes in this version.</div>';
    return;
  }
  const changed = [];
  for (let i = 0; i < lines.length; i++) if (lines[i].t !== "ctx") changed.push(i);
  if (changed.length === 0) {
    el.innerHTML = '<div class="rev-empty">No changes in this version.</div>';
    return;
  }
  const CTX = 3;
  const ranges = [];
  for (const idx of changed) {
    const lo = Math.max(0, idx - CTX);
    const hi = Math.min(lines.length - 1, idx + CTX);
    if (ranges.length && lo <= ranges[ranges.length - 1].hi + 1) {
      ranges[ranges.length - 1].hi = Math.max(ranges[ranges.length - 1].hi, hi);
    } else {
      ranges.push({ lo, hi });
    }
  }
  const frag = document.createDocumentFragment();
  ranges.forEach((r, k) => {
    if (k > 0) {
      const sep = document.createElement("div");
      sep.className = "diff-line diff-hunk";
      sep.textContent = "⋯";
      frag.appendChild(sep);
    }
    for (let i = r.lo; i <= r.hi; i++) {
      const ln = lines[i];
      const row = document.createElement("div");
      row.className = "diff-line";
      let sign = " ";
      if (ln.t === "add") { row.classList.add("diff-add"); sign = "+"; }
      else if (ln.t === "del") { row.classList.add("diff-del"); sign = "-"; }
      row.innerHTML = escapeHtml(sign + " " + ln.s) || "&nbsp;";
      frag.appendChild(row);
    }
  });
  el.appendChild(frag);
}

async function restoreSelectedRevision() {
  if (!revState || !revState.selected) return;
  const id = revState.id;
  const sha = revState.selected;
  const btn = document.getElementById("rev-restore");
  btn.disabled = true;
  try {
    await go.RestoreRevision(id, sha);
    closeRevisions();
    await loadAll();
    await openNote(id);
  } catch (e) {
    alert("Restore failed: " + (e && e.message ? e.message : e));
    btn.disabled = false;
  }
}

function updateSearchClear() {
  document.getElementById("search-wrap").classList.toggle("has-text", searchTerm.length > 0);
}

function updateFindClear() {
  document.querySelector(".find-wrap").classList.toggle("has-text", findTerm.length > 0);
}

function setupSplitters() {
  const limits = {
    nav: { min: 140, max: 320 },
    "list-col": { min: 200, max: 480 },
  };
  document.querySelectorAll(".splitter").forEach((sp) => {
    const targetId = sp.dataset.target;
    const lim = limits[targetId] || { min: 140, max: 600 };
    sp.addEventListener("mousedown", (e) => {
      e.preventDefault();
      const target = document.getElementById(targetId);
      const startX = e.clientX;
      const startW = target.getBoundingClientRect().width;
      function onMove(ev) {
        let w = startW + (ev.clientX - startX);
        w = Math.max(lim.min, Math.min(lim.max, w));
        target.style.width = w + "px";
      }
      function onUp() {
        document.removeEventListener("mousemove", onMove);
        document.removeEventListener("mouseup", onUp);
      }
      document.addEventListener("mousemove", onMove);
      document.addEventListener("mouseup", onUp);
    });
  });
}

document.getElementById("new-note").addEventListener("click", newNote);
document.getElementById("settings").addEventListener("click", openSettings);
document.getElementById("import-notes").addEventListener("click", (e) => {
  e.stopPropagation();
  const menu = document.getElementById("import-menu");
  if (menu.classList.contains("hidden")) openImportMenu();
  else closeImportMenu();
});
document.getElementById("preview").addEventListener("click", togglePreview);
document.getElementById("toggle-nav").addEventListener("click", toggleNav);
document.getElementById("list-back").addEventListener("click", () => setScreen("nav"));
document.getElementById("editor-back").addEventListener("click", () => setScreen("list"));
document.getElementById("editor-more").addEventListener("click", () =>
  document.body.classList.toggle("tools-open"),
);
detectPlatform();
setScreen("list");
document.getElementById("toggle-theme").addEventListener("click", toggleTheme);
document.getElementById("font-dec").addEventListener("click", () => changeFont(-1));
document.getElementById("font-inc").addEventListener("click", () => changeFont(1));
const tagInputEl = document.getElementById("tag-input");
tagInputEl.addEventListener("keydown", (e) => {
  if (e.key === "Enter" || e.key === "," || e.key === "Tab") {
    if (e.target.value.trim().length > 0) {
      e.preventDefault();
      commitTagInput();
      scheduleSave();
    }
  } else if (e.key === "Backspace" && e.target.value === "" && currentTags.length > 0) {
    currentTags.pop();
    renderTagChips();
    scheduleSave();
  }
});
tagInputEl.addEventListener("blur", () => {
  if (commitTagInput()) scheduleSave();
});
document.getElementById("tag-chips").addEventListener("click", (e) => {
  if (e.target.id === "tag-chips") tagInputEl.focus();
});

let searchTimer = null;
document.getElementById("search").addEventListener("input", (e) => {
  searchTerm = e.target.value;
  updateSearchClear();
  if (searchTimer) clearTimeout(searchTimer);
  searchTimer = setTimeout(loadAll, 150);
});
document.getElementById("search-clear").addEventListener("click", () => {
  searchTerm = "";
  document.getElementById("search").value = "";
  updateSearchClear();
  if (searchTimer) clearTimeout(searchTimer);
  loadAll();
});

document.getElementById("find").addEventListener("input", (e) => {
  findTerm = e.target.value;
  updateFindClear();
  applyFind();
});
document.getElementById("find-clear").addEventListener("click", () => {
  findTerm = "";
  document.getElementById("find").value = "";
  updateFindClear();
  applyFind();
});

document.querySelectorAll("#nav > .nav-item[data-view]").forEach((el) => {
  el.addEventListener("click", () => {
    selectView(el.dataset.view);
    document.getElementById("nav").focus();
  });
  el.addEventListener("contextmenu", (e) => {
    e.preventDefault();
    openExportMenu(e.clientX, e.clientY, el.dataset.view);
  });
});

document.querySelectorAll("#ctx-menu .ctx-item").forEach((el) => {
  el.addEventListener("click", () => contextAction(el.dataset.action));
});
document.querySelectorAll("#import-menu .ctx-item").forEach((el) => {
  el.addEventListener("click", () => importAction(el.dataset.action));
});
document.querySelectorAll("#export-menu .ctx-item").forEach((el) => {
  el.addEventListener("click", () => exportAction(el.dataset.action));
});
document.addEventListener("click", (e) => {
  if (!e.target.closest("#ctx-menu")) closeContextMenu();
  if (!e.target.closest("#import-menu") && !e.target.closest("#import-notes")) closeImportMenu();
  if (!e.target.closest("#export-menu")) closeExportMenu();
});
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") {
    closeContextMenu();
    closeImportMenu();
    closeExportMenu();
    if (!document.getElementById("revision-modal").classList.contains("hidden")) {
      closeRevisions();
    }
  }
  if (e.key !== "ArrowUp" && e.key !== "ArrowDown") return;
  const list = document.getElementById("note-list");
  const nav = document.getElementById("nav");
  const revList = document.getElementById("rev-list");
  const active = document.activeElement;
  if (revList && !document.getElementById("revision-modal").classList.contains("hidden") &&
      (active === revList || revList.contains(active))) {
    e.preventDefault();
    navigateRevisions(e.key === "ArrowDown" ? 1 : -1);
    return;
  }
  if (list && (active === list || list.contains(active))) {
    e.preventDefault();
    navigateNoteList(e.key === "ArrowDown" ? 1 : -1);
  } else if (nav && (active === nav || nav.contains(active))) {
    e.preventDefault();
    navigateNav(e.key === "ArrowDown" ? 1 : -1);
  }
});

function navigateRevisions(delta) {
  if (!revState || !revState.revs || revState.revs.length === 0) return;
  const revs = revState.revs;
  let idx = revs.findIndex((r) => r.sha === revState.selected);
  if (idx < 0) idx = delta > 0 ? -1 : revs.length;
  const next = Math.max(0, Math.min(revs.length - 1, idx + delta));
  if (revs[next].sha === revState.selected) return;
  const list = document.getElementById("rev-list");
  const item = list.querySelector('.rev-item[data-sha="' + revs[next].sha + '"]');
  if (item) {
    selectRevision(revs[next].sha, item);
    item.scrollIntoView({ block: "nearest" });
  }
}

function navigateNoteList(delta) {
  const visible = visibleNotes();
  if (visible.length === 0) return;
  let idx = visible.findIndex((n) => n.id === currentID);
  if (idx < 0) idx = delta > 0 ? -1 : visible.length;
  const next = Math.max(0, Math.min(visible.length - 1, idx + delta));
  if (visible[next].id === currentID) return;
  selectedIDs.clear();
  anchorID = visible[next].id;
  openNote(visible[next].id);
}

function navItemsInOrder() {
  return Array.from(document.querySelectorAll("#nav .nav-item[data-view]"));
}

function currentNavIndex(items) {
  return items.findIndex((el) => {
    const view = el.dataset.view;
    if (view === "tag") return activeView === "tag" && activeTag === el.dataset.tag;
    return activeView === view;
  });
}

function navigateNav(delta) {
  const items = navItemsInOrder();
  if (items.length === 0) return;
  let idx = currentNavIndex(items);
  if (idx < 0) idx = delta > 0 ? -1 : items.length;
  const next = Math.max(0, Math.min(items.length - 1, idx + delta));
  items[next].click();
}

document.getElementById("cfg-save").addEventListener("click", saveSettings);
document.getElementById("cfg-cancel").addEventListener("click", () => closeSettings());
document.getElementById("cfg-validate").addEventListener("click", validateSettings);
[
  "cfg-repourl",
  "cfg-server-url",
  "cfg-username",
  "cfg-useapi",
  "cfg-interval",
  "cfg-encryption-pin",
].forEach((id) => {
  const el = document.getElementById(id);
  el.addEventListener("input", updateSaveButtonState);
  el.addEventListener("change", updateSaveButtonState);
});
document.getElementById("cfg-token").addEventListener("input", () => {
  validatedFingerprint = null;
  updateSaveButtonState();
});
document.getElementById("settings-modal").addEventListener("click", (e) => {
  if (e.target.id === "settings-modal") closeSettings();
});

document.getElementById("rev-close").addEventListener("click", closeRevisions);
document.getElementById("rev-restore").addEventListener("click", restoreSelectedRevision);
document.getElementById("revision-modal").addEventListener("click", (e) => {
  if (e.target.id === "revision-modal") closeRevisions();
});
document.getElementById("rev-list").addEventListener("scroll", (e) => {
  const el = e.target;
  if (el.scrollTop + el.clientHeight >= el.scrollHeight - 40) loadRevisionPage();
});

initTheme();
initMermaid();
applyFontSize(getFontSize());
setupSplitters();
initEditor("");
if (go) {
  (async () => {
    await loadAll();
    const visible = visibleNotes();
    if (visible.length > 0) {
      await openNote(visible[0].id);
    }
    if (editor) editor.focus();
  })();
  listen("notes-changed", () => {
    if (document.getElementById("settings-modal").classList.contains("hidden")) {
      loadAll();
    }
  });
  listen("sync-error", (event) => {
    const error = String(event.payload?.message || event.payload || "");
    if (!error) return;
    const message = decryptionErrorMessage(error);
    const banner = document.getElementById("sync-error-msg");
    banner.textContent = message;
    banner.classList.remove("hidden");
  });
  listen("note-body-changed", async (event) => {
    const id = event.payload;
    if (!id || id !== currentID) return;
    if (saveTimer) { clearTimeout(saveTimer); saveTimer = null; }
    if (pushTimer) { clearTimeout(pushTimer); pushTimer = null; }
    try {
      const n = await go.LoadNote(id);
      setTags(n.tags || []);
      initEditor(n.body, !!n.trashed, n.filetype);
    } catch (e) {}
    await loadAll();
  });
} else {
  if (editor) editor.focus();
}
