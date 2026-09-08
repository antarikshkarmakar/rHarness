/* ============================================================
   Mythos Harness — Control Deck app
   Vanilla JS, no framework. Talks to the local Bun API.
   ============================================================ */

const $ = (sel) => document.querySelector(sel);
const $$ = (sel) => Array.from(document.querySelectorAll(sel));

const state = { running: false, runs: [] };

/* ── Tabs ─────────────────────────────────────────────────── */
function switchTab(name) {
  $$(".tab").forEach((t) => {
    const on = t.dataset.tab === name;
    t.classList.toggle("active", on);
    t.setAttribute("aria-selected", on ? "true" : "false");
  });
  $$(".view").forEach((v) => v.classList.toggle("active", v.id === `view-${name}`));
}
$$(".tab").forEach((t) => t.addEventListener("click", () => switchTab(t.dataset.tab)));

/* ── Health / provider badge ──────────────────────────────── */
async function pollHealth() {
  const dot = $("#health-dot");
  const badge = $("#provider-badge");
  try {
    const res = await fetch("/api/health");
    const data = await res.json();
    dot.classList.remove("err");
    dot.classList.add("ok");
    badge.textContent = data.provider === "demo" ? "demo mode" : `${data.model} · live`;
  } catch {
    dot.classList.add("err");
    badge.textContent = "offline";
  }
}

/* ── Messages ─────────────────────────────────────────────── */
const log = $("#chat-log");
const input = $("#chat-input");
const sendBtn = $("#send-btn");
const runBtn = $("#run-btn");
const stopBtn = $("#stop-btn");

const scrollDown = () => { log.scrollTop = log.scrollHeight; };

function addMessage(role, text) {
  const wrap = document.createElement("div");
  wrap.className = `msg ${role === "user" ? "msg-user" : ""}`;
  wrap.innerHTML = `
    <div class="msg-avatar">${role === "user" ? "You" : "r"}</div>
    <div class="msg-body">
      <div class="msg-name">${role === "user" ? "You" : "rHarness"}</div>
      <p class="msg-text"></p>
    </div>`;
  wrap.querySelector(".msg-text").textContent = text;
  log.appendChild(wrap);
  scrollDown();
  return wrap;
}

/* ── Runs ─────────────────────────────────────────────────── */
function countIterations(run) {
  return (run.events || []).filter((e) => e.type === "loop.iteration").length;
}

function addRun(run) {
  state.runs.unshift(run);
  if (state.runs.length > 50) state.runs.length = 50;
  renderRuns();
  if (run.status === "completed") {
    const bits = [
      `Task finished. ${countIterations(run)} iteration(s), ${run.artifacts?.length ?? 0} artifact(s).`,
    ];
    if (typeof run.confidence === "number") bits.push(`Confidence ${(run.confidence * 100).toFixed(0)}%.`);
    if (run.summary) bits.push(run.summary.replace(/\n/g, " · "));
    addMessage("assistant", bits.join(" "));
  } else if (run.status === "failed") {
    addMessage("assistant", "The finish-first loop ended without meeting success criteria. Check the Runs tab for detail — try again with a tighter task.");
  } else if (run.status === "cancelled") {
    addMessage("assistant", "Run cancelled.");
  }
}

function renderRuns() {
  const list = $("#run-list");
  if (!state.runs.length) {
    list.innerHTML = '<div class="empty-state">No runs yet — hit “Run task” in the Agent tab to kick off a finish-first loop.</div>';
    return;
  }
  list.innerHTML = state.runs.map((r, i) => {
    const dur = r.completed_at && r.started_at ? `${((new Date(r.completed_at) - new Date(r.started_at)) / 1000).toFixed(1)}s` : "—";
    const phases = [...new Set((r.phases || []).map((p) => p.phase))].join(" → ") || "—";
    return `
      <div class="run-card">
        <div class="run-task" data-task="${i}"></div>
        <span class="run-status status-${r.status}"><span class="dot"></span>${r.status}</span>
        <div class="run-detail">run ${r.id.slice(0, 8)} · ${countIterations(r)} iter · ${r.artifacts?.length ?? 0} artifacts · ${dur}</div>
        <div class="run-detail">phases: ${phases}</div>
      </div>`;
  }).join("");
  list.querySelectorAll("[data-task]").forEach((el) => {
    el.textContent = state.runs[Number(el.dataset.task)].task;
  });
}

function setRunning(on) {
  state.running = on;
  sendBtn.disabled = on;
  runBtn.disabled = on;
  stopBtn.disabled = !on;
  input.disabled = on;
}

/* ── Chat (quick reply) ───────────────────────────────────── */
async function sendChat() {
  const text = input.value.trim();
  if (!text || state.running) return;
  input.value = "";
  addMessage("user", text);
  setRunning(true);
  try {
    const res = await fetch("/api/chat", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ messages: [{ role: "user", content: text }] }),
    });
    const data = await res.json();
    if (data.ok) addMessage("assistant", data.reply || "(no reply)");
    else addMessage("assistant", data.error || "Something went wrong.");
  } catch (err) {
    addMessage("assistant", `Request failed: ${err.message}`);
  } finally {
    setRunning(false);
    input.focus();
  }
}

/* ── Run (finish-first loop) ──────────────────────────────── */
async function runTask() {
  const text = input.value.trim();
  if (!text || state.running) return;
  input.value = "";
  addMessage("user", `[finish-first] ${text}`);
  setRunning(true);
  try {
    const res = await fetch("/api/run", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ task: text }),
    });
    const data = await res.json();
    if (data.ok && data.run) addRun(data.run);
    else addMessage("assistant", data.error || "Run failed to start.");
  } catch (err) {
    addMessage("assistant", `Run failed: ${err.message}`);
  } finally {
    setRunning(false);
    input.focus();
  }
}

sendBtn.addEventListener("click", sendChat);
runBtn.addEventListener("click", runTask);
input.addEventListener("keydown", (e) => {
  if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); sendChat(); }
});
stopBtn.addEventListener("click", () => {
  addMessage("assistant", "Stop requested. (Reserved for long-running runs.)");
});

/* ── Plugins ──────────────────────────────────────────────── */
function renderPlugins(plugins) {
  const list = $("#plugin-list");
  $("#plugin-count").textContent = `${plugins.filter((p) => p.enabled).length}/${plugins.length} enabled`;
  if (!plugins.length) { list.innerHTML = '<div class="empty-state">No plugins registered.</div>'; return; }

  list.innerHTML = plugins.map((p) => `
    <div class="plugin-card ${p.enabled ? "" : "disabled"}" data-id="${p.id}">
      <div class="plugin-info">
        <div class="plugin-name">${p.name} <span class="plugin-ver">v${p.version}</span></div>
        <div class="plugin-desc"></div>
        <div class="plugin-tags">
          ${p.capabilities?.phases?.map((c) => `<span class="chip phase">${c}</span>`).join("") ?? ""}
        </div>
      </div>
      <label class="switch" title="${p.enabled ? "Disable" : "Enable"}">
        <input type="checkbox" ${p.enabled ? "checked" : ""} data-id="${p.id}" />
        <span class="slider"></span>
      </label>
    </div>`).join("");

  list.querySelectorAll(".plugin-card").forEach((card) => {
    const p = plugins.find((x) => x.id === card.dataset.id);
    if (p) card.querySelector(".plugin-desc").textContent = p.description;
  });

  list.querySelectorAll('input[type="checkbox"]').forEach((cb) => {
    cb.addEventListener("change", async () => {
      cb.disabled = true;
      try {
        await fetch("/api/plugins", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ id: cb.dataset.id, enabled: cb.checked }),
        });
        cb.closest(".plugin-card").classList.toggle("disabled", !cb.checked);
        const fresh = await (await fetch("/api/plugins")).json();
        $("#plugin-count").textContent = `${fresh.plugins.filter((p) => p.enabled).length}/${fresh.plugins.length} enabled`;
      } finally {
        cb.disabled = false;
      }
    });
  });
}

async function loadPlugins() {
  try {
    const res = await fetch("/api/plugins");
    const data = await res.json();
    renderPlugins(data.plugins || []);
  } catch (err) {
    $("#plugin-list").innerHTML = `<div class="empty-state">Failed to load plugins: ${err.message}</div>`;
  }
}

/* ── Boot ─────────────────────────────────────────────────── */
pollHealth();
setInterval(pollHealth, 8000);
loadPlugins();
renderRuns();
input.focus();
