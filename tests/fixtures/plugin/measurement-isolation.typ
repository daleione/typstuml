#import "lib.typ": render-puml, render-mermaid
#set page(width: 800pt, height: auto)
#let reused = render-mermaid("flowchart LR; A[Long label] -->|yes| B{Ready?}")
#text(size: 10pt, reused)
#text(size: 20pt, reused)
#render-puml("@startuml\nclass A\nA --> B : label\n@enduml")
#render-mermaid("flowchart TB; a-b[Short]; a_b[Very long label]")
#context {
  let values = query(<typstuml_measure>).map(it => it.value)
  let scopes = values.map(v => v.scope).dedup()
  if scopes.len() == 4 {
    let groups = scopes.map(s => values.filter(v => v.scope == s))
    for group in groups { assert.eq(group.map(v => v.id).dedup().len(), group.len()) }
    // Same content value in different font-size contexts gets independent widths.
    assert(groups.at(1).first().w > groups.at(0).first().w)
    assert(groups.at(3).at(1).w > groups.at(3).at(0).w)
    [#metadata((instances: scopes.len(), probes: values.len())) <result>]
  } else { [pending] }
}
