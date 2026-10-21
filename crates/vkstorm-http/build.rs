//! With the `link-curl` feature the crate names libcurl for the linker, for
//! its own tests and benches. `VKS_HTTP_CURL_LIB_DIR` adds a search path and
//! `VKS_HTTP_CURL_LIBS` (semicolon separated, each `name` or `static=name`)
//! replaces the default `curl`, so a prebuilt static libcurl with its TLS
//! libraries can be used instead of the system one. Without the feature
//! (the viewer build) nothing is emitted: the viewer's link supplies libcurl.

fn main() {
    println!("cargo:rerun-if-env-changed=VKS_HTTP_CURL_LIB_DIR");
    println!("cargo:rerun-if-env-changed=VKS_HTTP_CURL_LIBS");
    if std::env::var_os("CARGO_FEATURE_LINK_CURL").is_none() {
        return;
    }
    if let Ok(dir) = std::env::var("VKS_HTTP_CURL_LIB_DIR") {
        println!("cargo:rustc-link-search=native={dir}");
    }
    let libs = std::env::var("VKS_HTTP_CURL_LIBS").unwrap_or_else(|_| "curl".to_owned());
    for lib in libs.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        println!("cargo:rustc-link-lib={lib}");
    }
}
