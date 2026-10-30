//! Pasta do bundle da UI web embutido no `cthubd` (rust-embed, #199; spec da
//! PWA §8): `ui/dist-web` se existir (`npm run build:web`); senão uma página
//! de aviso gerada no `OUT_DIR`, para o `cargo build` e o `cargo test` do
//! workspace seguirem sem Node. Com `CTHUB_WEB_REQUIRED=1` (`cthubd update`
//! e o job `cthubd` da release), a falta do bundle faz o build falhar.
//!
//! O cargo só acompanha `ui/dist-web` quando ele existe: depois do primeiro
//! `npm run build:web` num checkout que compilou sem ele, rode
//! `touch crates/cthubd/build.rs` (ou `cargo clean -p cthubd`) para embutir.

use std::path::PathBuf;

const MISSING: &str = r#"<!doctype html>
<html lang="pt-BR">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Control Tower</title>
</head>
<body style="font-family: sans-serif; max-width: 40rem; margin: 3rem auto; padding: 0 1rem">
<h1>Control Tower</h1>
<p>Bundle web ausente: este <code>cthubd</code> foi compilado sem a UI web.
Rode <code>npm run build:web</code> em <code>ui/</code> e compile o <code>cthubd</code> de novo
(o <code>cthubd update</code> já faz isso).</p>
</body>
</html>
"#;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=CTHUB_WEB_REQUIRED");
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let dist = manifest.join("../../ui/dist-web");
    let required = std::env::var("CTHUB_WEB_REQUIRED").as_deref() == Ok("1");

    let (dir, present) = if dist.join("index.html").is_file() {
        println!("cargo:rerun-if-changed={}", dist.display());
        (dist.canonicalize().unwrap(), true)
    } else {
        if required {
            panic!(
                "CTHUB_WEB_REQUIRED=1, mas o bundle web não existe em {}: \
                 rode `npm ci && npm run build:web` em ui/",
                dist.display()
            );
        }
        // Sem `rerun-if-changed` na pasta ausente: o cargo trataria como
        // mudada a cada build e recompilaria o cthubd sempre.
        let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("web-missing");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("index.html"), MISSING).unwrap();
        (out, false)
    };
    println!("cargo:rustc-env=CTHUB_WEB_DIR={}", dir.display());
    println!(
        "cargo:rustc-env=CTHUB_WEB_BUNDLE={}",
        if present { "1" } else { "0" }
    );
}
