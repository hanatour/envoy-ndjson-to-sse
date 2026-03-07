# envoy-ndjson-to-sse

An Envoy proxy WASM filter that converts NDJSON (Newline Delimited JSON) response bodies into Server-Sent Events (SSE). Use it when a backend returns streaming NDJSON and you want clients to consume it as `text/event-stream` SSE.

## Behavior

- **Response headers**: Sets `Content-Type: text/event-stream` and removes `Content-Length`.
- **Body**: Each non-empty NDJSON line is emitted as one SSE event: `data: <trimmed-line>\n\n`.
- **Streaming**: Handles chunked response bodies; incomplete lines are buffered until a newline or end-of-stream.
- **Empty lines**: Lines that are empty or only whitespace are skipped.

## Build

Requires Rust and the `wasm32-wasip1` target:

```bash
rustup target add wasm32-wasip1
cargo build --target wasm32-wasip1 --release
```

Output: `target/wasm32-wasip1/release/envoy_ndjson_to_sse.wasm`

## Download pre-built WASM

- **Releases**: For each [release](https://github.com/hanatour/envoy-ndjson-to-sse/releases) (e.g. `v0.1.0`), download `envoy-ndjson-to-sse-<version>.wasm` from the release assets. Create a release by pushing a tag:
  ```bash
  git tag v0.1.0 && git push origin v0.1.0
  ```
- **CI artifacts**: The [Actions](https://github.com/hanatour/envoy-ndjson-to-sse/actions) workflow builds on every push to `master`. Open the latest successful run → **Summary** → download the `envoy-ndjson-to-sse-wasm` artifact (contains `envoy-ndjson-to-sse.wasm`).

## Envoy Configuration

Use the filter in the HTTP connection manager. Example snippet:

```yaml
http_filters:
  - name: envoy.filters.http.wasm
    typed_config:
      "@type": type.googleapis.com/envoy.extensions.filters.http.wasm.v3.Wasm
      config:
        name: ndjson_to_sse
        root_id: ndjson_to_sse
        vm_config:
          runtime: envoy.wasm.runtimes.v8
          code:
            local:
              filename: /path/to/envoy-ndjson-to-sse.wasm
  - name: envoy.filters.http.router
    typed_config:
      "@type": type.googleapis.com/envoy.extensions.filters.http.router.v3.Router
```

Point the route to a backend that returns NDJSON (e.g. `Content-Type: application/x-ndjson`). Clients can then request the same path and receive SSE.

## Testing

- **Unit tests** (NDJSON → SSE conversion, no Envoy):

  ```bash
  cargo test
  ```

- **Integration with Envoy**: See [TESTING.md](TESTING.md) for a full Envoy config example and curl usage.

## License

MIT License. See [LICENSE](LICENSE).
