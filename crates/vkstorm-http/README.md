# vkstorm-http

The viewer's HTTP layer: the `LLCore::HttpRequest` service of
`indra/llcorehttp` (request queue and service thread, policy classes with
connection limits and request-rate throttling, the retry queue with its
backoff, the libcurl multi-handle transport, the reply queues), as the
vkstorm-rust track's rung four. The viewer reaches it through `vks_http_*`
in `vkstorm-ffi`. The rung notes, with the transport judgement, the
boundary, the parity scenarios and the measurements, are in the viewer's
`doc/rust_port.md`.

## libcurl

The transport is libcurl, bound by hand in `src/sys.rs` (no `curl-sys`:
its build script finds a system libcurl or builds its own with the
platform TLS, neither being the libcurl the viewer stages with OpenSSL and
nghttp2). The crate itself names no library: the symbols resolve at the
viewer's link.

## Tests

- `cargo test -p vkstorm-http` (plain, no environment): the unit tests
  that do not touch libcurl: status codes and strings, header
  normalisation, `Content-Range` and `Retry-After` parsing, option
  clamping and validation, the retry backoff and queue order. In this
  build the libcurl entry points are stubs that panic if reached. This is
  what the workspace's `cargo test --workspace` runs.
- `cargo test -p vkstorm-http --features link-curl`: the full set, with
  `tests/scenarios.rs` (16 scenarios against the crate's loopback mock
  server in `tests/mock`) and the unit tests linked against a real
  libcurl. The feature makes `build.rs` name the library:
  - `VKS_HTTP_CURL_LIB_DIR` adds a search path and `VKS_HTTP_CURL_LIBS`
    (semicolon separated, `name` or `static=name`) names the libraries.
    To test against the viewer's own prebuilt libcurl, point them at a
    viewer build's `packages/lib/release`:

    Windows (Git Bash):

        export VKS_HTTP_CURL_LIB_DIR='C:\path\to\build-vc170-64\packages\lib\release'
        export VKS_HTTP_CURL_LIBS='static=libcurl;static=libssl;static=libcrypto;static=nghttp2;static=zlib;ws2_32;crypt32;wldap32;normaliz;advapi32;user32;gdi32'
        cargo test -p vkstorm-http --features link-curl

    Linux:

        export VKS_HTTP_CURL_LIB_DIR=$HOME/path/to/build-linux-64/packages/lib/release
        export VKS_HTTP_CURL_LIBS='static=curl;static=ssl;static=crypto;static=nghttp2;static=z'
        cargo test -p vkstorm-http --features link-curl

  - Without the variables the feature links the system `libcurl`
    (`-lcurl`, needs the development package on Linux, e.g.
    `libcurl4-openssl-dev`), which is a different libcurl version from the
    product's and is fine for the crate's own tests only.
- `cargo bench -p vkstorm-http --features link-curl --bench loopback`:
  requests per second and latency against the mock server (small LLSD GET
  and POST, a 1 MiB range download); `VKS_HTTP_BENCH_ITERATIONS` sets the
  count.
