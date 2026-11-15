fn main() {
    println!("cargo:rerun-if-changed=src/pw_capture.c");

    let pw = pkg_config::Config::new()
        .cargo_metadata(false)
        .atleast_version("0.3")
        .probe("libpipewire-0.3")
        .expect("libpipewire-0.3 development headers are required to build steamos-crd-bridge");

    let mut build = cc::Build::new();
    build.file("src/pw_capture.c");
    for path in &pw.include_paths {
        build.include(path);
    }
    build.flag_if_supported("-O3");
    build.flag_if_supported("-Wno-unused-parameter");
    build.compile("pw_capture");

    // Emit dynamic link directives AFTER static `libpw_capture.a` so GNU ld resolves symbols in order
    for path in &pw.link_paths {
        println!("cargo:rustc-link-search=native={}", path.display());
    }
    for lib in &pw.libs {
        println!("cargo:rustc-link-lib={lib}");
    }
}
