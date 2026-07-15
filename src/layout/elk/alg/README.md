# Ported ELK `layered` sources — EPL-2.0

Everything under `src/layout/elk/alg/` is a Rust port of the Eclipse
Layout Kernel's `layered` algorithm and is licensed under the
**Eclipse Public License 2.0** (see `LICENSE.md` in this directory),
NOT the MIT license that covers the rest of this repository.

- Upstream: <https://github.com/eclipse-elk/elk>, tag `v0.11.0`
  (matching the elkjs 0.11.x oracle in `tools/elk-oracle/`).
- Source plugin: `plugins/org.eclipse.elk.alg.layered/`.
- Each Rust file names the Java file(s) it was ported from in its
  module doc comment.

Porting policy (docs/elk-port-plan.md): faithful, line-by-line where
behavior lives; Rust-idiomatic only in mechanics (arena indices
instead of object references, `Option` instead of `null`). Every phase
is verified numerically against elkjs via `tests/elk_port.rs`.
