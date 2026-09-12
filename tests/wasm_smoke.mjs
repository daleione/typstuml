// Run after crates/typstuml-wasm/build.sh. Exercise the actual JS/WASM boundary.
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import init, {
  renderSvgAs, renderPngAs, renderPdfAs, emitTypstAs, renderSvg,
} from '../crates/typstuml-wasm/pkg/typstuml_wasm.js';

const wasm = new URL('../crates/typstuml-wasm/pkg/typstuml_wasm_bg.wasm', import.meta.url);
await init({ module_or_path: await readFile(wasm) });
const source = 'flowchart RL; A[Start] -->|yes| B{Ready?} --> C((Done))';

test('Mermaid SVG and Typst use the independent flowchart painter', () => {
  assert.match(renderSvgAs(source, 'mermaid'), /<svg/);
  const typst = emitTypstAs(source, 'mermaid');
  assert.match(typst, /#flowchart-layout\(/);
  assert.match(typst, /flow-diamond/);
});

test('Mermaid binary exports produce PNG and PDF', () => {
  assert.deepEqual([...renderPngAs(source, 'mermaid', 1).slice(0, 8)], [137, 80, 78, 71, 13, 10, 26, 10]);
  assert.equal(new TextDecoder().decode(renderPdfAs(source, 'mermaid').slice(0, 5)), '%PDF-');
});

test('legacy PlantUML export remains available', () => {
  assert.match(renderSvg('@startuml\nAlice -> Bob: Hi\n@enduml'), /<svg/);
});

test('invalid language and unsupported Mermaid diagrams fail explicitly', () => {
  assert.throws(() => renderSvgAs(source, 'unknown'), /unknown input language/);
  assert.throws(() => renderSvgAs('sequenceDiagram\nA->>B: Hi', 'mermaid'));
});
