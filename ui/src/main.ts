import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import type { ConnectionDto, ExportRequest, ForwardInfo, NodeDto, SessionState } from "./api";
import { backend, inTauri } from "./backend";
import { closeLeaf, type Dir, leaves, newTab, type Pane, splitLeaf, type Tab } from "./layout";
import "./style.css";
import { addFolder, allConnections, exposedForwards, formatForwards, parseForwards, filterTree, findConnection, folderNames, folderOf, newId, removeConnection, upsertConnection, validate } from "./tree";

interface View {
  leafId: string;
  sessionId: string;
  conn: ConnectionDto;
  el: HTMLElement;
  term: Terminal;
  fit: FitAddon;
  unlisten: (() => void)[];
  closed: boolean;
  status: HTMLElement;
}

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
let nodes: NodeDto[] = [];
const tabs: Tab[] = [];
let activeTab = "";
const views = new Map<string, View>(); // by leaf id
let seq = 0;
const uid = (p: string) => `${p}${++seq}-${Math.random().toString(36).slice(2, 7)}`;

// ---------- sidebar ----------
function renderTree() {
  const root = $("tree");
  root.replaceChildren();
  const q = $<HTMLInputElement>("search").value;
  const build = (list: NodeDto[], into: HTMLElement) => {
    for (const n of list) {
      if (n.type === "folder") {
        const f = document.createElement("div");
        f.className = "folder";
        f.innerHTML = `<div class="label">▾ ${esc(n.name)}</div><div class="children"></div>`;
        const kids = f.querySelector<HTMLElement>(".children")!;
        f.querySelector(".label")!.addEventListener("click", () => (kids.hidden = !kids.hidden));
        build(n.children, kids);
        into.append(f);
      } else {
        const c = document.createElement("div");
        c.className = "conn";
        c.dataset.testid = `conn-${n.name}`;
        c.title = `${n.protocol}://${n.username}@${n.host}${n.port ? ":" + n.port : ""}`;
        c.innerHTML = `<span>${esc(n.name)}</span><span class="proto">${n.protocol}</span>${
          n.protocol === "telnet" ? '<span class="warn" title="Telnet is not encrypted">⚠</span>' : ""
        }<span class="spacer"></span><span class="act" data-testid="edit-${esc(n.name)}" title="Edit">✎</span><span class="act" data-testid="delete-${esc(n.name)}" title="Delete">✕</span>`;
        c.addEventListener("dblclick", () => void openConnection(n));
        c.querySelector('[title="Edit"]')!.addEventListener("click", (e) => {
          e.stopPropagation();
          openEditor(n);
        });
        c.querySelector('[title="Delete"]')!.addEventListener("click", (e) => {
          e.stopPropagation();
          if (confirm(`Delete "${n.name}"?`)) void persist(removeConnection(nodes, n.id));
        });
        into.append(c);
      }
    }
  };
  build(filterTree(nodes, q), root);
}

function esc(s: string) {
  return s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);
}

async function persist(next: NodeDto[]) {
  nodes = next;
  renderTree();
  try {
    await backend.saveTree(next);
  } catch (e) {
    toast(`Could not save connections: ${String(e)}`, true);
  }
}

// ---------- connection editor ----------
function openEditor(existing?: ConnectionDto) {
  const m = $("modal");
  const jumpCandidates = allConnections(nodes).filter((c) => c.protocol === "ssh" && c.id !== existing?.id);
  const folders = folderNames(nodes);
  const cur = existing ? folderOf(nodes, existing.id) ?? null : null;
  m.hidden = false;
  m.innerHTML = `<form class="box" data-testid="editor" novalidate>
    <h3>${existing ? "Edit" : "New"} connection</h3>
    <label>Name</label><input name="name" data-testid="edit-name" />
    <span></span><span class="err" data-err="name"></span>
    <label>Protocol</label><select name="protocol" data-testid="edit-protocol">
      ${["ssh", "sftp", "scp", "telnet"].map((p) => `<option>${p}</option>`).join("")}</select>
    <label>Host</label><input name="host" data-testid="edit-host" />
    <span class="err" data-err="host"></span>
    <label>Port</label><input name="port" data-testid="edit-port" placeholder="default" />
    <span class="err" data-err="port"></span>
    <label>User</label><input name="username" data-testid="edit-user" />
    <label>Secret from</label><select name="provider" data-testid="edit-provider">
      <option value="">(ask / none)</option><option>bitwarden</option><option>keepassxc</option><option>file</option></select>
    <label>Secret item</label><input name="item" data-testid="edit-item" placeholder="item id" />
    <span class="err" data-err="secret"></span>
    <label>Secret kind</label><select name="kind" data-testid="edit-kind"><option>password</option><option>keyfile</option></select>
    <label>Jump hosts</label><select name="jump" data-testid="edit-jump" multiple size="3">
      ${jumpCandidates.map((c) => `<option value="${esc(c.id)}">${esc(c.name)}</option>`).join("")}</select>
    <label>Forwards</label><textarea name="forwards" rows="3" data-testid="edit-forwards" placeholder="one per line, like ssh:&#10;L 5433:db.internal:5432&#10;R 9000:localhost:3000&#10;D 1080"></textarea>
    <span></span><span class="err" data-err="forwards" data-testid="edit-forwards-err"></span>
    <span></span><span class="warn" data-testid="edit-forwards-warn"></span>
    <label>On disconnect</label><select name="reconnect" data-testid="edit-reconnect">
      <option value="">Default (auto with forwards, otherwise ask)</option><option value="auto">Reconnect automatically</option>
      <option value="ask">Ask me</option><option value="off">Close the session</option></select>
    <label>Tunnel only</label><label><input type="checkbox" name="no_shell" data-testid="edit-noshell" /> No interactive shell (like ssh -N)</label>
    <label>Folder</label><select name="folder" data-testid="edit-folder">
      <option value="">(top level)</option>${folders.map((f) => `<option>${esc(f)}</option>`).join("")}</select>
    <div class="buttons"><button type="button" data-testid="edit-cancel">Cancel</button>
    <button type="submit" data-testid="edit-save">Save</button></div></form>`;
  const form = m.querySelector("form")!;
  const f = (n: string) => form.elements.namedItem(n) as HTMLInputElement & HTMLSelectElement;
  if (existing) {
    f("name").value = existing.name;
    f("protocol").value = existing.protocol;
    f("host").value = existing.host;
    f("port").value = existing.port ? String(existing.port) : "";
    f("username").value = existing.username;
    f("provider").value = existing.secret?.provider ?? "";
    f("item").value = existing.secret?.item ?? "";
    f("kind").value = existing.secret?.kind ?? "password";
    for (const o of Array.from(f("jump").options)) o.selected = existing.jump?.includes(o.value) ?? false;
    f("folder").value = cur ?? "";
    (form.elements.namedItem("forwards") as HTMLTextAreaElement).value = formatForwards(existing.forwards);
    f("reconnect").value = existing.reconnect ?? "";
    f("no_shell").checked = existing.no_shell ?? false;
  }
  const fwText = () => (form.elements.namedItem("forwards") as HTMLTextAreaElement).value;
  const updateFwWarn = () => {
    const warn = m.querySelector<HTMLElement>('[data-testid="edit-forwards-warn"]')!;
    warn.textContent = exposedForwards(parseForwards(fwText()).forwards).length ? "Some forwards listen on a non-loopback address and are reachable from other machines." : "";
  };
  form.addEventListener("input", updateFwWarn);
  updateFwWarn();
  const close = () => {
    m.hidden = true;
    m.replaceChildren();
  };
  m.querySelector('[data-testid="edit-cancel"]')!.addEventListener("click", close);
  form.addEventListener("submit", (ev) => {
    ev.preventDefault();
    const port = f("port").value.trim();
    const conn: ConnectionDto = {
      type: "connection",
      id: existing?.id ?? newId(),
      name: f("name").value.trim(),
      protocol: f("protocol").value as ConnectionDto["protocol"],
      host: f("host").value.trim(),
      username: f("username").value.trim(),
      ...(port ? { port: Number(port) } : {}),
      jump: Array.from(f("jump").selectedOptions).map((o) => o.value),
      ...(f("provider").value
        ? { secret: { provider: f("provider").value, item: f("item").value.trim(), kind: f("kind").value as "password" | "keyfile" } }
        : {}),
    };
    const fw = parseForwards(fwText());
    if (fw.forwards.length) conn.forwards = fw.forwards;
    if (f("reconnect").value) conn.reconnect = f("reconnect").value as "auto" | "ask" | "off";
    if (f("no_shell").checked) conn.no_shell = true;
    const errors: Record<string, string> = { ...validate(conn), ...(fw.errors.length ? { forwards: fw.errors.join("; ") } : {}) };
    form.querySelectorAll<HTMLElement>("[data-err]").forEach((el) => (el.textContent = errors[el.dataset.err!] ?? ""));
    if (Object.keys(errors).length) return;
    close();
    void persist(upsertConnection(nodes, conn, f("folder").value || null));
  });
  f("name").focus();
}

// ---------- connection state ----------
function renderStatus(v: View, st: SessionState) {
  const bar = v.status;
  const btn = (label: string, id: string, fn: () => void) => {
    const b = document.createElement("button");
    b.textContent = label;
    b.dataset.testid = id;
    b.addEventListener("click", fn);
    bar.append(b);
  };
  bar.replaceChildren();
  bar.hidden = true;
  if (st.state === "reconnecting") {
    bar.hidden = false;
    const t = document.createElement("span");
    t.dataset.testid = "status-text";
    t.textContent = `Reconnecting… attempt ${st.attempt}${st.nextInSecs ? `, next try in ${st.nextInSecs}s` : ""}`;
    bar.append(t);
    btn("Reconnect now", "reconnect-now", () => void backend.reconnectSession(v.sessionId));
    btn("Close", "status-close", () => closeLeafById(v.leafId));
  } else if (st.state === "lost") {
    bar.hidden = false;
    const t = document.createElement("span");
    t.dataset.testid = "status-text";
    t.textContent = `Connection lost: ${st.reason}`;
    bar.append(t);
    btn("Reconnect", "reconnect", () => void backend.reconnectSession(v.sessionId));
    btn("Close", "status-close", () => closeLeafById(v.leafId));
  }
}

function closeLeafById(leafId: string) {
  const t = tabs.find((x) => leaves(x.root).some((l) => l.id === leafId));
  if (!t) return;
  t.focus = leafId;
  activeTab = t.id;
  closePane();
}

// ---------- forwards panel ----------
function openForwards() {
  const v = focusedView();
  if (!v) return;
  const m = $("modal");
  m.hidden = false;
  m.innerHTML = `<div class="box wide" role="dialog" data-testid="forwards-dialog">
    <h3>Port forwards — ${esc(v.conn.name)}</h3>
    <table><thead><tr><th>Type</th><th>Listen</th><th>Destination</th><th>Status</th><th></th></tr></thead>
      <tbody data-testid="forwards-rows"></tbody></table>
    <form data-testid="forward-form" class="row" novalidate>
      <select name="kind" data-testid="fw-kind"><option value="local">Local (-L)</option><option value="remote">Remote (-R)</option><option value="dynamic">SOCKS (-D)</option></select>
      <input name="bind_host" placeholder="bind address (127.0.0.1)" data-testid="fw-bind" size="22" />
      <input name="bind_port" placeholder="port" data-testid="fw-port" size="6" />
      <input name="dest_host" placeholder="dest host" data-testid="fw-dest-host" size="16" />
      <input name="dest_port" placeholder="dest port" data-testid="fw-dest-port" size="6" />
      <button type="submit" data-testid="fw-add">Add</button>
    </form>
    <div class="err" data-testid="fw-error"></div>
    <div class="warn" data-testid="fw-warn"></div>
    <div class="buttons"><button type="button" data-testid="fw-close">Close</button></div></div>`;
  const rows = m.querySelector<HTMLElement>('[data-testid="forwards-rows"]')!;
  const form = m.querySelector("form")!;
  const f = (n: string) => form.elements.namedItem(n) as HTMLInputElement & HTMLSelectElement;
  const q = (id: string) => m.querySelector<HTMLElement>(`[data-testid="${id}"]`)!;
  const show = (list: ForwardInfo[]) => {
    rows.replaceChildren();
    for (const fw of list) {
      const tr = document.createElement("tr");
      const dest = fw.spec.kind === "dynamic" ? "SOCKS5 proxy" : `${fw.spec.dest_host}:${fw.spec.dest_port}`;
      const listen = `${fw.spec.bind_host ?? "127.0.0.1"}:${fw.state === "listening" ? fw.port : fw.spec.bind_port}`;
      const status = fw.state === "listening" ? `listening (${fw.connections} conn.)` : `failed: ${fw.reason}`;
      tr.innerHTML = `<td>${fw.spec.kind}</td><td>${esc(listen)}</td><td>${esc(dest)}</td><td class="${fw.state === "failed" ? "failed" : ""}" data-testid="fw-status">${esc(status)}</td><td><button type="button" data-testid="fw-stop-${fw.id}">Stop</button></td>`;
      tr.querySelector("button")!.addEventListener("click", () => void backend.forwardStop(v.sessionId, fw.id).then(refresh));
      rows.append(tr);
    }
    q("fw-warn").textContent = exposedForwards(list.map((x) => x.spec)).length ? "Some forwards listen on a non-loopback address and are reachable from other machines." : "";
  };
  const refresh = () => backend.forwardList(v.sessionId).then(show, () => show([]));
  const un = backend.onForwards(v.sessionId, show);
  const timer = setInterval(() => void refresh(), 1500);
  const close = () => {
    clearInterval(timer);
    un();
    m.hidden = true;
    m.replaceChildren();
  };
  q("fw-close").addEventListener("click", close);
  form.addEventListener("submit", (e) => {
    e.preventDefault();
    const kind = f("kind").value as "local" | "remote" | "dynamic";
    const text = `${kind === "local" ? "L" : kind === "remote" ? "R" : "D"} ${f("bind_host").value.trim() ? f("bind_host").value.trim() + ":" : ""}${f("bind_port").value.trim()}${kind === "dynamic" ? "" : `:${f("dest_host").value.trim()}:${f("dest_port").value.trim()}`}`;
    const parsed = parseForwards(text);
    q("fw-error").textContent = parsed.errors.join("; ");
    const spec = parsed.forwards[0];
    if (!spec) return;
    backend.forwardAdd(v.sessionId, spec).then(
      () => {
        form.reset();
        void refresh();
      },
      (err: unknown) => (q("fw-error").textContent = String(err instanceof Error ? err.message : err)),
    );
  });
  void refresh();
}

// ---------- log capture ----------
const recording = new Set<string>(); // leaf ids

function focusedView(): View | undefined {
  const t = currentTab();
  return t ? views.get(t.focus) : undefined;
}

function refreshRecUi() {
  const v = focusedView();
  const on = !!v && recording.has(v.leafId);
  $("rec").classList.toggle("on", on);
  $("rec").textContent = on ? "■ Stop recording" : "● Record";
  for (const [id, view] of views) view.el.classList.toggle("recording", recording.has(id));
}

async function toggleRecording() {
  const v = focusedView();
  if (!v) return;
  if (recording.has(v.leafId)) {
    await backend.captureStop(v.sessionId);
    recording.delete(v.leafId);
    toast("Recording stopped. Use “Export log…” to save or copy it.");
  } else {
    await backend.captureStart(v.sessionId);
    recording.add(v.leafId);
  }
  refreshRecUi();
}

function openExport() {
  const v = focusedView();
  if (!v) return;
  const m = $("modal");
  m.hidden = false;
  m.innerHTML = `<div class="box wide" role="dialog"><form data-testid="export-dialog" novalidate>
    <h3>Export log — ${esc(v.conn.name)}</h3>
    <div class="row">
      <label><input type="radio" name="scope" value="all" checked data-testid="scope-all" /> Everything</label>
      <label><input type="radio" name="scope" value="last" data-testid="scope-last" /> Last
        <input name="lastN" type="number" min="1" value="1" style="width:60px" data-testid="last-n" /> command(s)</label>
      <label><input type="checkbox" name="strip" checked data-testid="opt-strip" /> Strip colors/escapes</label>
    </div>
    <div class="row">Anonymize:
      <label><input type="checkbox" name="usernames" data-testid="anon-users" /> usernames</label>
      <label><input type="checkbox" name="hostnames" data-testid="anon-hosts" /> hostnames &amp; IPs</label>
      <label><input type="checkbox" name="secrets" data-testid="anon-secrets" /> passwords &amp; keys</label>
    </div>
    <div>Custom regex (one per line)
      <textarea name="custom" rows="2" data-testid="anon-custom" placeholder="e.g. ACME-\d+"></textarea>
      <div class="err" data-testid="regex-errors"></div></div>
    <details><summary>Prompt pattern (advanced)</summary>
      <input name="prompt" style="width:100%" data-testid="prompt-regex" placeholder="default detects bash, PowerShell, Cisco…" /></details>
    <div class="note" data-testid="export-note" style="color:var(--warn)"></div>
    <textarea class="preview" readonly data-testid="export-preview"></textarea>
    <div class="map" data-testid="export-map"></div>
    <div class="buttons"><button type="button" data-testid="export-copy">Copy to clipboard</button>
      <button type="button" data-testid="export-save">Save to file…</button>
      <button type="button" data-testid="export-close">Close</button></div></form></div>`;
  const form = m.querySelector("form")!;
  const f = (n: string) => form.elements.namedItem(n) as HTMLInputElement;
  const q = (id: string) => m.querySelector<HTMLElement>(`[data-testid="${id}"]`)!;

  const request = (): ExportRequest => {
    const anyAnon = f("usernames").checked || f("hostnames").checked || f("secrets").checked;
    const custom = (form.elements.namedItem("custom") as HTMLTextAreaElement).value.split("\n").map((l) => l.trim()).filter(Boolean);
    return {
      scope: (form.elements.namedItem("scope") as unknown as RadioNodeList).value === "last" ? "last" : "all",
      lastN: Math.max(1, Number(f("lastN").value) || 1),
      stripEscapes: f("strip").checked,
      ...(f("prompt").value.trim() ? { promptRegex: f("prompt").value.trim() } : {}),
      ...(anyAnon || custom.length
        ? { anonymize: { usernames: f("usernames").checked, hostnames: f("hostnames").checked, secrets: f("secrets").checked, custom } }
        : {}),
    };
  };

  let seq = 0;
  const refresh = async () => {
    const mine = ++seq;
    const req = request();
    const errs: string[] = [];
    for (const p of req.anonymize?.custom ?? []) {
      const e = await backend.captureRegexError(p);
      if (e) errs.push(`${p}: ${e}`);
    }
    if (req.promptRegex) {
      const e = await backend.captureRegexError(req.promptRegex);
      if (e) errs.push(`prompt pattern: ${e}`);
    }
    q("regex-errors").textContent = errs.join("\n");
    if (errs.length) return;
    try {
      const r = await backend.capturePreview(v.sessionId, req);
      if (mine !== seq) return;
      (q("export-preview") as HTMLTextAreaElement).value = r.text;
      q("export-note").textContent = r.fallbackToLines ? "No shell prompt recognised; showing the last lines instead." : "";
      q("export-map").textContent = r.mapping.map(([a, b]) => `${a} → ${b}`).join("\n");
    } catch (e) {
      (q("export-preview") as HTMLTextAreaElement).value = "";
      q("export-note").textContent = String(e instanceof Error ? e.message : e);
    }
  };
  form.addEventListener("input", () => void refresh());
  form.addEventListener("change", () => void refresh());
  const close = () => {
    m.hidden = true;
    m.replaceChildren();
  };
  q("export-close").addEventListener("click", close);
  q("export-copy").addEventListener("click", () => {
    void backend.copyText((q("export-preview") as HTMLTextAreaElement).value).then(() => toast("Copied to clipboard"));
  });
  q("export-save").addEventListener("click", () => {
    void (async () => {
      const path = await backend.pickSavePath(`${v.conn.name}.log`);
      if (!path) return;
      try {
        await backend.captureSave(v.sessionId, request(), path);
        toast(`Saved ${path}`);
      } catch (e) {
        toast(`Save failed: ${String(e)}`, true);
      }
    })();
  });
  void refresh();
}

// ---------- sessions / views ----------
function createView(conn: ConnectionDto): View {
  const leafId = uid("leaf");
  const sessionId = uid("sess");
  const el = document.createElement("div");
  el.className = "leaf";
  el.dataset.testid = `leaf-${leafId}`;
  const holder = document.createElement("div");
  holder.className = "term";
  el.append(holder);
  const term = new Terminal({ fontFamily: "Cascadia Mono, Consolas, monospace", fontSize: 14, cursorBlink: true, scrollback: 10000 });
  const fit = new FitAddon();
  term.loadAddon(fit);
  term.open(holder);
  const status = document.createElement("div");
  status.className = "statusbar";
  status.dataset.testid = "statusbar";
  status.hidden = true;
  el.append(status);
  const v: View = { leafId, sessionId, conn, el, term, fit, unlisten: [], closed: false, status };
  term.onData((d) => void backend.write(sessionId, d));
  term.onResize(({ cols, rows }) => void backend.resize(sessionId, cols, rows));
  v.unlisten.push(
    backend.onData(sessionId, (b) => term.write(b)),
    backend.onClosed(sessionId, (reason) => {
      v.closed = true;
      const m = document.createElement("div");
      m.className = "closed";
      m.textContent = `session closed: ${reason}`;
      el.append(m);
    }),
  );
  v.unlisten.push(backend.onSessionState(sessionId, (st) => renderStatus(v, st)));
  el.addEventListener("mousedown", () => focusLeaf(leafId));
  wireDrop(v);
  const ro = new ResizeObserver(() => requestAnimationFrame(() => safeFit(v)));
  ro.observe(holder);
  v.unlisten.push(() => ro.disconnect());
  views.set(leafId, v);
  return v;
}

function safeFit(v: View) {
  if (v.el.isConnected && v.el.clientWidth > 0) v.fit.fit();
}

function connect(v: View) {
  const { cols, rows } = v.term;
  backend.openSession(v.sessionId, v.conn.id, cols, rows).catch((e: unknown) => {
    v.closed = true;
    v.term.write(`\r\n\x1b[31mConnection failed: ${String(e instanceof Error ? e.message : e)}\x1b[0m\r\n`);
  });
}

async function openConnection(conn: ConnectionDto) {
  const v = createView(conn);
  const t = newTab(uid("tab"), conn.name, { id: v.leafId, sessionId: v.sessionId });
  tabs.push(t);
  activeTab = t.id;
  showBanner(conn);
  render();
  connect(v);
}

function showBanner(conn: ConnectionDto) {
  const b = $("banner");
  b.hidden = conn.protocol !== "telnet";
  b.textContent = conn.protocol === "telnet" ? `⚠ ${conn.name}: Telnet is not encrypted; passwords are sent in clear text.` : "";
}

function currentTab() {
  return tabs.find((t) => t.id === activeTab);
}

function focusLeaf(leafId: string) {
  const t = currentTab();
  if (!t || t.focus === leafId) return;
  t.focus = leafId;
  markFocus();
  views.get(leafId)?.term.focus();
}

function markFocus() {
  queueMicrotask(refreshRecUi);
  const t = currentTab();
  for (const l of document.querySelectorAll<HTMLElement>(".leaf")) l.classList.remove("focus");
  if (t) views.get(t.focus)?.el.classList.add("focus");
}

function split(dir: Dir) {
  const t = currentTab();
  if (!t) return;
  const src = views.get(t.focus);
  if (!src) return;
  const v = createView(src.conn);
  const i = tabs.indexOf(t);
  tabs[i] = splitLeaf(t, t.focus, dir, { id: v.leafId, sessionId: v.sessionId });
  render();
  connect(v);
}

function closePane() {
  const t = currentTab();
  if (!t) return;
  const v = views.get(t.focus);
  if (v) disposeView(v);
  const next = closeLeaf(t, t.focus);
  const i = tabs.indexOf(t);
  if (next) tabs[i] = next;
  else {
    tabs.splice(i, 1);
    activeTab = tabs[Math.max(0, i - 1)]?.id ?? "";
  }
  render();
}

function closeTab(id: string) {
  const t = tabs.find((x) => x.id === id);
  if (!t) return;
  for (const l of leaves(t.root)) {
    const v = views.get(l.id);
    if (v) disposeView(v);
  }
  const i = tabs.indexOf(t);
  tabs.splice(i, 1);
  if (activeTab === id) activeTab = tabs[Math.max(0, i - 1)]?.id ?? "";
  render();
}

function disposeView(v: View) {
  recording.delete(v.leafId);
  v.unlisten.forEach((u) => u());
  void backend.closeSession(v.sessionId);
  v.term.dispose();
  v.el.remove();
  views.delete(v.leafId);
}

// ---------- rendering ----------
function render() {
  renderTabs();
  const host = $("panes");
  host.replaceChildren();
  const t = currentTab();
  if (!t) {
    host.innerHTML = '<div class="empty">Double-click a connection to open a session.</div>';
    $("banner").hidden = true;
    return;
  }
  host.append(buildPane(t.root));
  markFocus();
  requestAnimationFrame(() => {
    for (const l of leaves(t.root)) {
      const v = views.get(l.id);
      if (v) safeFit(v);
    }
    views.get(t.focus)?.term.focus();
  });
}

function renderTabs() {
  const bar = $("tabbar");
  bar.replaceChildren();
  for (const t of tabs) {
    const el = document.createElement("div");
    el.className = "tab" + (t.id === activeTab ? " active" : "");
    el.dataset.testid = `tab-${t.title}`;
    el.innerHTML = `<span>${esc(t.title)}</span><span class="x" data-testid="tab-close">✕</span>`;
    el.addEventListener("click", () => {
      activeTab = t.id;
      const v = views.get(t.focus);
      if (v) showBanner(v.conn);
      render();
    });
    el.querySelector(".x")!.addEventListener("click", (e) => {
      e.stopPropagation();
      closeTab(t.id);
    });
    bar.append(el);
  }
}

function buildPane(p: Pane): HTMLElement {
  if (p.kind === "leaf") {
    const v = views.get(p.id)!;
    v.el.style.flex = "1 1 0";
    return v.el;
  }
  const box = document.createElement("div");
  box.className = `split ${p.dir}`;
  const a = buildPane(p.a);
  const b = buildPane(p.b);
  const div = document.createElement("div");
  div.className = "divider";
  div.dataset.testid = "divider";
  const apply = () => {
    a.style.flex = `${p.ratio} 1 0`;
    b.style.flex = `${1 - p.ratio} 1 0`;
  };
  apply();
  div.addEventListener("mousedown", (e) => {
    e.preventDefault();
    const rect = box.getBoundingClientRect();
    const move = (m: MouseEvent) => {
      const r = p.dir === "row" ? (m.clientX - rect.left) / rect.width : (m.clientY - rect.top) / rect.height;
      p.ratio = Math.min(0.9, Math.max(0.1, r));
      apply();
    };
    const up = () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
      render();
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  });
  box.append(a, div, b);
  return box;
}

// ---------- drag and drop upload ----------
function upload(v: View, paths: string[]) {
  for (const path of paths) {
    void backend.upload(v.sessionId, path, ".").catch((e: unknown) => toast(`Upload failed: ${String(e)}`, true));
  }
}

function wireDrop(v: View) {
  // Browser / mock: DOM drop events. In Tauri the OS drop arrives via onDragDropEvent below.
  v.el.addEventListener("dragover", (e) => {
    e.preventDefault();
    v.el.classList.add("dropping");
  });
  v.el.addEventListener("dragleave", () => v.el.classList.remove("dropping"));
  v.el.addEventListener("drop", (e) => {
    e.preventDefault();
    v.el.classList.remove("dropping");
    const files = Array.from(e.dataTransfer?.files ?? []);
    upload(v, files.map((f) => (f as File & { path?: string }).path ?? f.name));
  });
}

async function wireTauriDrop() {
  const { getCurrentWebview } = await import("@tauri-apps/api/webview");
  const at = (x: number, y: number) => {
    const el = document.elementFromPoint(x / devicePixelRatio, y / devicePixelRatio)?.closest<HTMLElement>(".leaf");
    return [...views.values()].find((v) => v.el === el);
  };
  await getCurrentWebview().onDragDropEvent((e) => {
    const p = e.payload;
    if (p.type === "leave") views.forEach((v) => v.el.classList.remove("dropping"));
    else if (p.type === "over") views.forEach((v) => v.el.classList.toggle("dropping", v === at(p.position.x, p.position.y)));
    else if (p.type === "drop") {
      views.forEach((v) => v.el.classList.remove("dropping"));
      const v = at(p.position.x, p.position.y);
      if (v) upload(v, p.paths);
    }
  });
}

// ---------- host key prompt, toasts ----------
function toast(msg: string, error = false, id?: string) {
  const box = $("toasts");
  let el = id ? box.querySelector<HTMLElement>(`[data-id="${id}"]`) : null;
  if (!el) {
    el = document.createElement("div");
    if (id) el.dataset.id = id;
    box.append(el);
  }
  el.className = "toast" + (error ? " error" : "");
  el.textContent = msg;
  return el;
}

backend.onProgress((p) => {
  if (p.error) toast(`Transfer failed: ${p.error}`, true, p.transferId);
  else if (p.finished) {
    const el = toast("Transfer complete", false, p.transferId);
    setTimeout(() => el.remove(), 4000);
  } else toast(`Transferring… ${p.total ? Math.round((100 * p.done) / p.total) : 0}%`, false, p.transferId);
});

backend.onHostKeyPrompt((p) => {
  const m = $("modal");
  m.hidden = false;
  m.innerHTML = `<div class="box" role="dialog">
    <h3>Unknown host key</h3>
    <div>The authenticity of <b>${esc(p.host)}:${p.port}</b> can't be established. Key:</div>
    <code>${esc(p.key)}</code>
    <button data-testid="hostkey-trust">Trust and connect</button>
    <button data-testid="hostkey-reject">Reject</button></div>`;
  const answer = (trust: boolean) => {
    m.hidden = true;
    m.replaceChildren();
    void backend.answerHostKey(p.promptId, trust);
  };
  m.querySelector('[data-testid="hostkey-trust"]')!.addEventListener("click", () => answer(true));
  m.querySelector('[data-testid="hostkey-reject"]')!.addEventListener("click", () => answer(false));
});

backend.onCredentialPrompt((p) => {
  const m = $("modal");
  m.hidden = false;
  m.innerHTML = `<form class="box" data-testid="password-dialog">
    <h3>${p.kind === "unlock" ? `Unlock ${esc(p.provider ?? "vault")}` : "Password required"}</h3>
    <div>${
      p.kind === "unlock"
        ? `Master password needed to read credentials for ${esc(p.hop)}.`
        : `${esc(p.username)}@${esc(p.host)} (${esc(p.hop)})`
    }</div>
    ${p.error ? `<div class="err" data-testid="password-error">${esc(p.error)}</div>` : ""}
    <input type="password" name="pw" data-testid="password-input" autocomplete="off" style="margin:8px 0;width:100%" />
    <button type="submit" data-testid="password-ok">Connect</button>
    <button type="button" data-testid="password-cancel">Cancel</button></form>`;
  const form = m.querySelector("form")!;
  const done = (secret: string | null) => {
    m.hidden = true;
    m.replaceChildren();
    void backend.answerCredential(p.promptId, secret);
  };
  form.addEventListener("submit", (e) => {
    e.preventDefault();
    done((form.elements.namedItem("pw") as HTMLInputElement).value);
  });
  m.querySelector('[data-testid="password-cancel"]')!.addEventListener("click", () => done(null));
  (form.elements.namedItem("pw") as HTMLInputElement).focus();
});

// ---------- boot ----------
$("split-row").addEventListener("click", () => split("row"));
$("split-col").addEventListener("click", () => split("col"));
$("close-pane").addEventListener("click", closePane);
$("rec").addEventListener("click", () => void toggleRecording());
$("forwards-btn").addEventListener("click", openForwards);
$("export-log").addEventListener("click", openExport);
$("search").addEventListener("input", renderTree);
$("new-conn").addEventListener("click", () => openEditor());
$("import-ssh").addEventListener("click", () => {
  backend
    .importSshConfig()
    .then(async (n) => {
      nodes = await backend.loadTree();
      renderTree();
      toast(n ? `Imported ${n} host${n === 1 ? "" : "s"} from ssh config` : "No hosts found in ssh config");
    })
    .catch((e: unknown) => toast(`Import failed: ${String(e)}`, true));
});
$("new-folder").addEventListener("click", () => {
  const name = prompt("Folder name");
  if (name) void persist(addFolder(nodes, name));
});

void (async () => {
  nodes = await backend.loadTree();
  renderTree();
  render();
  if (inTauri) await wireTauriDrop();
})();

if (!inTauri) {
  // Test hooks for the browser/mock mode only.
  (window as unknown as Record<string, unknown>).__simpu = {
    backend,
    screen: (leafId?: string) => {
      const t = currentTab();
      const v = views.get(leafId ?? t?.focus ?? "");
      if (!v) return "";
      const b = v.term.buffer.active;
      return Array.from({ length: b.length }, (_, i) => b.getLine(i)?.translateToString(true) ?? "").join("\n");
    },
    leafCount: () => document.querySelectorAll(".leaf").length,
    findConnection: (id: string) => findConnection(nodes, id),
  };
}
