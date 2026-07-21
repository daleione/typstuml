// TypstUML interactive tree viewer.
//
// Division of labour (docs/mindmap-web-interactive-design.md §4):
//   - JS measures node sizes ONCE (the browser's font engine is ground
//     truth for what it renders) — including nodes hidden by folding.
//   - Rust (wasm `treeLayout`) computes coordinates on every fold.
//   - JS paints an SVG keyed by stable node IDs and animates deltas.
//
// The renderer intentionally reproduces components/src/tree.typ's visual
// vocabulary (rounded rect / underline nodes, elbow polylines, no
// arrowheads) rather than markmap's — only markmap's interaction
// patterns are borrowed (fold state on the data, toggle bound to the
// circle only, recursive toggle via meta key, enter/exit anchored to
// the toggled node).

import init, { treeModel, treeLayout } from "../../crates/typstuml-wasm/pkg/typstuml_wasm.js";

const EM = 10; // SVG user units per em; font-size matches (style.css).
const INSET_X = 0.8 * EM;
const INSET_Y = 0.4 * EM;
const LINE_H = 1.2 * EM;
const DEFAULT_FILL = "#90CAF9"; // palettes.pastel.blue
const DURATION = 300;

const svg = document.getElementById("canvas");
const viewport = document.getElementById("viewport");
const edgesG = document.getElementById("edges");
const nodesG = document.getElementById("nodes");
const statusEl = document.getElementById("status");
const titleEl = document.getElementById("title");

const NS = "http://www.w3.org/2000/svg";

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

let model = null;        // parsed model JSON (object)
let modelStr = null;     // the exact string handed back to treeLayout
let nodesById = new Map();// id -> model node (+ .dir annotation)
let parentOf = new Map(); // id -> parent id
let sizes = {};          // id -> [w, h] measured once
let folded = new Set();  // node ids whose children are pruned
let nodeEls = new Map(); // id -> <g>
let edgeEls = new Map(); // "from-to" -> <polyline>
let lastRects = new Map();// id -> {x, y} previous layout (animation origins)
let view = { x: 40, y: 40, k: 1 };
let edgeAnimFrame = null;

// ---------------------------------------------------------------------------
// Model handling
// ---------------------------------------------------------------------------

function indexModel() {
  nodesById.clear();
  parentOf.clear();
  const isMindmap = model.kind === "mindmap";
  const walk = (node, parent, dir) => {
    node.dir = dir;
    nodesById.set(node.id, node);
    if (parent !== null) parentOf.set(node.id, parent);
    for (const c of node.children) {
      // Mindmap: the first level fixes the branch direction; deeper
      // levels inherit it. WBS: everything grows down.
      let childDir = dir;
      if (isMindmap && node.id === model.root.id) {
        childDir = c.side === "left" ? "left" : "right";
      }
      walk(c, node.id, childDir);
    }
  };
  walk(model.root, null, isMindmap ? "right" : "down");
}

// ---------------------------------------------------------------------------
// Measurement — once per load, all nodes (folded ones included), so the
// fold loop never re-measures. Uses the same text construction as the
// live renderer, guaranteeing measure == render.
// ---------------------------------------------------------------------------

function measureAll() {
  const probe = document.createElementNS(NS, "g");
  probe.setAttribute("class", "tree-node");
  probe.setAttribute("visibility", "hidden");
  nodesG.appendChild(probe);
  sizes = {};
  for (const node of nodesById.values()) {
    const text = buildText(node, 0);
    probe.appendChild(text);
    let maxW = 0;
    for (const tspan of text.children) {
      maxW = Math.max(maxW, tspan.getComputedTextLength());
    }
    probe.removeChild(text);
    const w = maxW + 2 * INSET_X;
    const h = node.label.length * LINE_H + 2 * INSET_Y;
    sizes[node.id] = [w, h];
  }
  nodesG.removeChild(probe);
}

// ---------------------------------------------------------------------------
// SVG builders
// ---------------------------------------------------------------------------

function buildText(node, w) {
  const text = document.createElementNS(NS, "text");
  text.setAttribute("text-anchor", "middle");
  node.label.forEach((line, i) => {
    const tspan = document.createElementNS(NS, "tspan");
    tspan.setAttribute("x", w / 2);
    // Vertically center line i inside the box.
    tspan.setAttribute("y", INSET_Y + (i + 0.5) * LINE_H);
    tspan.setAttribute("dominant-baseline", "central");
    tspan.textContent = line;
    text.appendChild(tspan);
  });
  return text;
}

function buildNodeEl(node, rect) {
  const g = document.createElementNS(NS, "g");
  g.setAttribute("class", "tree-node");
  g.dataset.id = node.id;

  if (node.shape === "underline") {
    const line = document.createElementNS(NS, "line");
    line.setAttribute("class", "underline");
    line.setAttribute("x1", 0);
    line.setAttribute("y1", rect.h);
    line.setAttribute("x2", rect.w);
    line.setAttribute("y2", rect.h);
    g.appendChild(line);
  } else {
    const box = document.createElementNS(NS, "rect");
    box.setAttribute("class", "box");
    box.setAttribute("width", rect.w);
    box.setAttribute("height", rect.h);
    box.setAttribute("rx", 3);
    box.setAttribute("fill", node.fill || DEFAULT_FILL);
    g.appendChild(box);
  }

  g.appendChild(buildText(node, rect.w));

  if (node.children.length > 0) {
    g.appendChild(buildToggle(node, rect));
  }
  return g;
}

// Fold affordance: a small circle on the node's outward edge (bottom
// for down-trees, left/right for mindmap branches). Not part of the
// Typst visual language — it exists only on the interactive surface.
function buildToggle(node, rect) {
  const t = document.createElementNS(NS, "g");
  t.setAttribute("class", "fold-toggle");
  const pos =
    node.dir === "down" ? [rect.w / 2, rect.h]
    : node.dir === "left" ? [0, rect.h / 2]
    : [rect.w, rect.h / 2];
  const c = document.createElementNS(NS, "circle");
  c.setAttribute("cx", pos[0]);
  c.setAttribute("cy", pos[1]);
  c.setAttribute("r", 3.2);
  t.appendChild(c);
  const count = document.createElementNS(NS, "text");
  count.setAttribute("x", pos[0]);
  count.setAttribute("y", pos[1] + 2.1);
  t.appendChild(count);
  const title = document.createElementNS(NS, "title");
  t.appendChild(title);
  t.addEventListener("click", (ev) => {
    ev.stopPropagation();
    toggleFold(node.id, ev.metaKey || ev.ctrlKey);
  });
  t.addEventListener("mousedown", (ev) => ev.stopPropagation());
  return t;
}

function refreshToggle(node) {
  const g = nodeEls.get(node.id);
  if (!g) return;
  const t = g.querySelector(".fold-toggle");
  if (!t) return;
  const isFolded = folded.has(node.id);
  t.classList.toggle("folded", isFolded);
  const descendants = countDescendants(node);
  t.querySelector("text").textContent = isFolded ? String(Math.min(descendants, 99)) : "";
  t.querySelector("title").textContent = isFolded
    ? `unfold (${descendants} hidden)`
    : "fold";
}

function countDescendants(node) {
  let n = 0;
  const walk = (m) => { for (const c of m.children) { n += 1; walk(c); } };
  walk(node);
  return n;
}

// ---------------------------------------------------------------------------
// Layout + render
// ---------------------------------------------------------------------------

function relayout() {
  const dl = JSON.parse(
    treeLayout(modelStr, JSON.stringify(sizes), JSON.stringify([...folded]), EM),
  );
  return dl;
}

function render(dl, originId) {
  // Animation anchors: entering content grows out of the toggled node's
  // OLD position; exiting content shrinks toward its NEW position.
  const originOld = originId !== undefined ? lastRects.get(originId) : undefined;
  const newRects = new Map(dl.nodes.map((n) => [n.id, n]));
  const originNew = originId !== undefined ? newRects.get(originId) : undefined;

  // --- nodes ---
  const seen = new Set();
  for (const n of dl.nodes) {
    seen.add(n.id);
    let g = nodeEls.get(n.id);
    if (!g) {
      const node = nodesById.get(n.id);
      g = buildNodeEl(node, n);
      const from = originOld || n;
      g.style.opacity = "0";
      setTransform(g, from.x, from.y);
      nodesG.appendChild(g);
      // Flush so the transition animates from the origin.
      void g.getBBox();
      g.style.opacity = "1";
    }
    setTransform(g, n.x, n.y);
    g.style.opacity = "1";
  }
  // Remove exiting nodes.
  for (const [id, g] of collectEls(nodesG, "tree-node")) {
    if (seen.has(id)) continue;
    const to = originNew || lastRects.get(id) || { x: 0, y: 0 };
    setTransform(g, to.x, to.y);
    g.style.opacity = "0";
    setTimeout(() => g.remove(), DURATION);
  }
  // Rebuild the id -> element map from the live DOM.
  nodeEls = collectEls(nodesG, "tree-node");

  // --- edges ---
  const seenEdges = new Set();
  const oldPts = new Map();
  for (const [key, el] of edgeEls) oldPts.set(key, parsePoints(el.getAttribute("points")));
  for (const e of dl.edges) {
    const key = `${e.from}-${e.to}`;
    seenEdges.add(key);
    let el = edgeEls.get(key);
    if (!el) {
      el = document.createElementNS(NS, "polyline");
      el.setAttribute("class", "tree-edge");
      el.dataset.key = key;
      const anchor = originOld ? [[originOld.x, originOld.y]] : e.points;
      el.setAttribute("points", formatPoints(collapseTo(e.points, anchor)));
      el.style.opacity = "0";
      edgesG.appendChild(el);
      void el.getBBox();
      el.style.opacity = "1";
      oldPts.set(key, parsePoints(el.getAttribute("points")));
    }
    el.dataset.target = JSON.stringify(e.points);
  }
  for (const [key, el] of edgeEls) {
    if (seenEdges.has(key)) continue;
    el.style.opacity = "0";
    setTimeout(() => el.remove(), DURATION);
    edgeEls.delete(key);
  }
  edgeEls = collectEls(edgesG, "tree-edge", "key");
  animateEdges(oldPts);

  // Refresh toggles (fold state / counts).
  for (const node of nodesById.values()) refreshToggle(node);

  lastRects = newRects;
}

function collectEls(container, cls, dataKey = "id") {
  const map = new Map();
  for (const el of container.querySelectorAll(`.${cls.replace(/ /g, ".")}`)) {
    if (el.style.opacity === "0") continue; // exiting
    const raw = el.dataset[dataKey];
    map.set(dataKey === "id" ? Number(raw) : raw, el);
  }
  return map;
}

function setTransform(g, x, y) {
  g.style.transform = `translate(${x}px, ${y}px)`;
}

// --- polyline morph -------------------------------------------------------

function parsePoints(str) {
  if (!str) return [];
  return str.trim().split(/\s+/).map((p) => p.split(",").map(Number));
}
function formatPoints(pts) {
  return pts.map((p) => `${p[0]},${p[1]}`).join(" ");
}
// Pad `pts` to `n` points by repeating the last one.
function padTo(pts, n) {
  const out = pts.slice();
  while (out.length < n) out.push(out[out.length - 1]);
  return out;
}
function collapseTo(shapePts, anchorPts) {
  // A brand-new edge starts life collapsed at the anchor point.
  return shapePts.map(() => anchorPts[0]);
}

function animateEdges(oldPts) {
  if (edgeAnimFrame) cancelAnimationFrame(edgeAnimFrame);
  const start = performance.now();
  const jobs = [];
  for (const [key, el] of edgeEls) {
    const target = JSON.parse(el.dataset.target);
    const from = padTo(oldPts.get(key) || target, target.length);
    const to = padTo(target, from.length);
    jobs.push({ el, from, to });
  }
  const tick = (now) => {
    const t = Math.min(1, (now - start) / DURATION);
    const ease = t < 0.5 ? 2 * t * t : 1 - (-2 * t + 2) ** 2 / 2;
    for (const { el, from, to } of jobs) {
      const pts = to.map((p, i) => [
        from[i][0] + (p[0] - from[i][0]) * ease,
        from[i][1] + (p[1] - from[i][1]) * ease,
      ]);
      el.setAttribute("points", formatPoints(pts));
    }
    if (t < 1) edgeAnimFrame = requestAnimationFrame(tick);
    else {
      // Snap to the exact target (drops padding duplicates).
      for (const { el } of jobs) {
        el.setAttribute("points", formatPoints(JSON.parse(el.dataset.target)));
      }
      edgeAnimFrame = null;
    }
  };
  edgeAnimFrame = requestAnimationFrame(tick);
}

// ---------------------------------------------------------------------------
// Interaction
// ---------------------------------------------------------------------------

function toggleFold(id, recursive) {
  const node = nodesById.get(id);
  if (!node || node.children.length === 0) return;
  const willFold = !folded.has(id);
  if (recursive) {
    const walk = (m) => {
      if (m.children.length > 0) {
        if (willFold) folded.add(m.id);
        else folded.delete(m.id);
      }
      for (const c of m.children) walk(c);
    };
    walk(node);
  } else if (willFold) {
    folded.add(id);
  } else {
    folded.delete(id);
  }
  render(relayout(), id);
}

function applyView() {
  viewport.setAttribute(
    "transform",
    `translate(${view.x}, ${view.y}) scale(${view.k})`,
  );
}

function fit() {
  if (!lastRects.size) return;
  const dlW = Math.max(...[...lastRects.values()].map((r) => r.x + r.w));
  const dlH = Math.max(...[...lastRects.values()].map((r) => r.y + r.h));
  const { clientWidth: cw, clientHeight: ch } = svg;
  const pad = 40;
  const k = Math.min((cw - 2 * pad) / dlW, (ch - 2 * pad) / dlH, 2.5);
  view = { k, x: (cw - dlW * k) / 2, y: (ch - dlH * k) / 2 };
  applyView();
}

function setupPanZoom() {
  let panning = null;
  svg.addEventListener("mousedown", (ev) => {
    panning = { x: ev.clientX, y: ev.clientY, vx: view.x, vy: view.y };
    svg.classList.add("panning");
  });
  window.addEventListener("mousemove", (ev) => {
    if (!panning) return;
    view.x = panning.vx + (ev.clientX - panning.x);
    view.y = panning.vy + (ev.clientY - panning.y);
    applyView();
  });
  window.addEventListener("mouseup", () => {
    panning = null;
    svg.classList.remove("panning");
  });
  svg.addEventListener(
    "wheel",
    (ev) => {
      ev.preventDefault();
      if (ev.ctrlKey || ev.metaKey) {
        // Zoom about the cursor (pinch gestures arrive as ctrl+wheel).
        const rect = svg.getBoundingClientRect();
        const mx = ev.clientX - rect.left;
        const my = ev.clientY - rect.top;
        const factor = Math.exp(-ev.deltaY * 0.01);
        const k = Math.min(8, Math.max(0.1, view.k * factor));
        view.x = mx - ((mx - view.x) / view.k) * k;
        view.y = my - ((my - view.y) / view.k) * k;
        view.k = k;
      } else {
        view.x -= ev.deltaX;
        view.y -= ev.deltaY;
      }
      applyView();
    },
    { passive: false },
  );
}

// ---------------------------------------------------------------------------
// Load
// ---------------------------------------------------------------------------

function load(source) {
  statusEl.textContent = "";
  try {
    modelStr = treeModel(source);
  } catch (e) {
    statusEl.textContent = String(e.message || e);
    return;
  }
  model = JSON.parse(modelStr);
  folded = new Set();
  nodesG.replaceChildren();
  edgesG.replaceChildren();
  nodeEls = new Map();
  edgeEls = new Map();
  lastRects = new Map();
  titleEl.textContent = model.title || "";
  indexModel();
  measureAll();
  render(relayout());
  fit();
}

async function main() {
  await init();
  document.getElementById("btn-load").addEventListener("click", () => {
    load(document.getElementById("source").value);
  });
  document.getElementById("btn-fit").addEventListener("click", fit);
  document.getElementById("btn-expand-all").addEventListener("click", () => {
    folded.clear();
    render(relayout());
  });
  setupPanZoom();

  // ?src=<url> preloads a .puml from the same origin (e.g.
  // ?src=/tests/fixtures/wbs/colors.puml when serving the repo root).
  const srcUrl = new URLSearchParams(location.search).get("src");
  if (srcUrl) {
    try {
      const text = await (await fetch(srcUrl)).text();
      document.getElementById("source").value = text;
    } catch (e) {
      statusEl.textContent = `fetch ${srcUrl}: ${e}`;
    }
  }
  load(document.getElementById("source").value);

  // Headless self-test hook (?selftest=1): exercise the fold loop and
  // report element counts into #status for a --dump-dom assertion.
  if (new URLSearchParams(location.search).get("selftest")) {
    const count = () => ({
      nodes: nodesG.querySelectorAll(".tree-node").length,
      edges: edgesG.querySelectorAll(".tree-edge").length,
    });
    const results = [];
    const initial = count();
    results.push(`initial n=${initial.nodes} e=${initial.edges}`);
    // Fold the first foldable non-root node.
    const target = [...nodesById.values()].find(
      (n) => n.id !== model.root.id && n.children.length > 0,
    );
    toggleFold(target.id, false);
    await new Promise((r) => setTimeout(r, 2 * DURATION));
    const foldedC = count();
    results.push(`folded(${target.id}) n=${foldedC.nodes} e=${foldedC.edges}`);
    toggleFold(target.id, false);
    await new Promise((r) => setTimeout(r, 2 * DURATION));
    const back = count();
    results.push(`unfolded n=${back.nodes} e=${back.edges}`);
    toggleFold(model.root.id, true); // recursive fold-all from root
    await new Promise((r) => setTimeout(r, 2 * DURATION));
    const all = count();
    results.push(`fold-all n=${all.nodes} e=${all.edges}`);
    statusEl.textContent = `SELFTEST ${results.join(" | ")}`;
  }
}

main();
