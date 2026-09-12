#import "../lib.typ": render-mermaid, render-puml
#set page(width: 800pt, height: auto)
#let source = "flowchart LR; A[Long label] -->|yes| B{Ready?}"
#let reused = render-mermaid(source)
#text(size: 10pt, reused)
#text(size: 20pt, reused)
#render-puml("@startuml\nclass A\nA --> B : test\n@enduml")
#render-mermaid("flowchart TB; a-b[Short]; a_b[Very long label]")
