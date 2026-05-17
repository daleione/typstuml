use std::io::Read;

fn main() {
    let mut src = String::new();
    std::io::stdin().read_to_string(&mut src).expect("read stdin");
    let out = std::env::args()
        .nth(1)
        .expect("usage: render-typst <output.png|.svg>");
    let format = if out.ends_with(".svg") {
        typstuml::runtime::Format::Svg
    } else {
        typstuml::runtime::Format::Png { scale: 2.0 }
    };
    let rendered = typstuml::runtime::render(src, None, format)
        .unwrap_or_else(|e| panic!("render failed: {e:?}"));
    std::fs::write(&out, &rendered.bytes).expect("write output");
    eprintln!("wrote {out}");
}
