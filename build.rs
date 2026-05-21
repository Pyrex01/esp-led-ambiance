fn main() {
    linker_be_nice();
    generate_brotli_assets();
    // make sure linkall.x is the last linker script (otherwise might cause problems with flip-link)
    println!("cargo:rustc-link-arg=-Tlinkall.x");
}

fn generate_brotli_assets() {
    use brotli::CompressorReader;
    use std::fs;
    use std::io::{Read, Write};
    use std::path::PathBuf;
    use walkdir::WalkDir;

    println!("cargo:rerun-if-changed=web/dist");

    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let dist_dir = manifest_dir.join("web").join("dist");
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let asset_dir = out_dir.join("web_assets");
    let generated = out_dir.join("generated_assets.rs");

    fs::create_dir_all(&asset_dir).unwrap();

    let mut entries = Vec::new();
    if dist_dir.exists() {
        for entry in WalkDir::new(&dist_dir)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
        {
            let path = entry.path();
            println!("cargo:rerun-if-changed={}", path.display());

            let relative = path.strip_prefix(&dist_dir).unwrap();
            let route = format!("/{}", relative.to_string_lossy().replace('\\', "/"));
            let safe_name = route
                .trim_start_matches('/')
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                .collect::<String>();
            let compressed_path = asset_dir.join(format!("{safe_name}.br"));

            let bytes = fs::read(path).unwrap();
            let mut compressed = Vec::new();
            let mut compressor = CompressorReader::new(bytes.as_slice(), 4096, 11, 22);
            compressor.read_to_end(&mut compressed).unwrap();
            fs::write(&compressed_path, compressed).unwrap();

            entries.push((route, content_type(path), compressed_path));
        }
    }

    if entries.is_empty() {
        let fallback = br#"<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>ESP32-S3</title></head><body><h1>Build web/ first</h1><p>Run npm install && npm run build in web/, then rebuild the firmware.</p></body></html>"#;
        let compressed_path = asset_dir.join("index_html.br");
        let mut compressed = Vec::new();
        let mut compressor = CompressorReader::new(fallback.as_slice(), 4096, 11, 22);
        compressor.read_to_end(&mut compressed).unwrap();
        fs::write(&compressed_path, compressed).unwrap();
        entries.push(("/index.html".to_string(), "text/html; charset=utf-8", compressed_path));
    }

    entries.sort_by(|left, right| left.0.cmp(&right.0));

    let mut output = String::from(
        "pub struct Asset {\n    pub path: &'static str,\n    pub content_type: &'static str,\n    pub bytes: &'static [u8],\n}\n\npub static ASSETS: &[Asset] = &[\n",
    );
    for (route, content_type, compressed_path) in entries {
        output.push_str(&format!(
            "    Asset {{ path: {:?}, content_type: {:?}, bytes: include_bytes!({:?}) }},\n",
            route,
            content_type,
            compressed_path.display().to_string()
        ));
    }
    output.push_str("];\n");

    fs::File::create(generated)
        .unwrap()
        .write_all(output.as_bytes())
        .unwrap();
}

fn content_type(path: &std::path::Path) -> &'static str {
    match path.extension().and_then(|ext| ext.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "txt" => "text/plain; charset=utf-8",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

fn linker_be_nice() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        let kind = &args[1];
        let what = &args[2];

        match kind.as_str() {
            "undefined-symbol" => match what.as_str() {
                what if what.starts_with("_defmt_") => {
                    eprintln!();
                    eprintln!(
                        "💡 `defmt` not found - make sure `defmt.x` is added as a linker script and you have included `use defmt_rtt as _;`"
                    );
                    eprintln!();
                }
                "_stack_start" => {
                    eprintln!();
                    eprintln!("💡 Is the linker script `linkall.x` missing?");
                    eprintln!();
                }
                what if what.starts_with("esp_rtos_") => {
                    eprintln!();
                    eprintln!(
                        "💡 `esp-radio` has no scheduler enabled. Make sure you have initialized `esp-rtos` or provided an external scheduler."
                    );
                    eprintln!();
                }
                "embedded_test_linker_file_not_added_to_rustflags" => {
                    eprintln!();
                    eprintln!(
                        "💡 `embedded-test` not found - make sure `embedded-test.x` is added as a linker script for tests"
                    );
                    eprintln!();
                }
                "free"
                | "malloc"
                | "calloc"
                | "get_free_internal_heap_size"
                | "malloc_internal"
                | "realloc_internal"
                | "calloc_internal"
                | "free_internal" => {
                    eprintln!();
                    eprintln!(
                        "💡 Did you forget the `esp-alloc` dependency or didn't enable the `compat` feature on it?"
                    );
                    eprintln!();
                }
                _ => (),
            },
            // we don't have anything helpful for "missing-lib" yet
            _ => {
                std::process::exit(1);
            }
        }

        std::process::exit(0);
    }

    println!(
        "cargo:rustc-link-arg=-Wl,--error-handling-script={}",
        std::env::current_exe().unwrap().display()
    );
}
