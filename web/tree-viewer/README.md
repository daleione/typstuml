# TypstUML interactive tree viewer

Browser-side renderer for `@startmindmap` / `@startwbs` diagrams with
fold/unfold, pan and zoom. Design rationale and architecture:
[`docs/mindmap-web-interactive-design.md`](../../docs/mindmap-web-interactive-design.md).

Division of labour:

- **JS measures** every node's rendered size once (the browser's font
  engine is ground truth for what it paints), folded nodes included —
  the fold loop never re-measures.
- **Rust lays out** (`treeLayout` wasm export → `src/layout/tree.rs`,
  the same code the CLI uses). Pure arithmetic per fold, no Typst.
- **JS paints** an SVG keyed by stable node IDs, mirroring the Typst
  painter's visual vocabulary (rounded rect / underline nodes, elbow
  polylines, `#90CAF9` default fill, no arrowheads).

## Run

```sh
# 1. Build the wasm package (once, and after Rust changes)
cd crates/typstuml-wasm && ./build.sh --no-opt

# 2. Serve the repo root (ES modules + wasm need HTTP)
python3 -m http.server 8123

# 3. Open
open http://127.0.0.1:8123/web/tree-viewer/index.html
```

Query params:

- `?src=/tests/fixtures/mindmap/colors.puml` — preload a `.puml` from
  the same origin.
- `?selftest=1` — run the headless fold-loop self-test; results land in
  the `#status` element (asserted via `chrome --headless --dump-dom`).

## Interaction

- **○ circle** on a node's outward edge: click to fold / unfold that
  subtree; ⌘/Ctrl-click folds recursively. When folded, the circle
  fills dark and shows the hidden-descendant count. (The circle is the
  only click target so node text stays selectable — a markmap lesson.)
- **Mindmap root** gets one circle per populated side; each folds only
  its own column (⌘/Ctrl-click additionally collapses every branch
  inside that column). A WBS root's single circle still collapses the
  whole tree.
- **Drag** pans; **⌘/Ctrl + wheel** (or pinch) zooms about the cursor;
  plain wheel pans (macOS convention).
- **fit** re-centers the diagram; **expand all** clears every fold.
