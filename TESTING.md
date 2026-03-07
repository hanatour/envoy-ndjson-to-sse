# WASM Filter Testing Guide

## 1. Unit Tests (Conversion Logic)

Validates the NDJSON → SSE conversion functions on the host only. Does not run WASM.

```bash
cargo test
```

- Includes 4 tests for `ndjson_to_sse`: single/multiple lines, empty line handling, empty input, etc.

## 2. Integration Testing with Envoy (Real WASM Execution)

Run the filter in Envoy, request a backend that returns NDJSON, and verify that the response is SSE.

### Prerequisites

- **Envoy**: Binary with WASM extension support (e.g. official images `envoyproxy/envoy` or `envoyproxy/envoy-dev`).
- **WASM file**:  
  `cargo build --target wasm32-wasip1 --release`  
  → `target/wasm32-wasip1/release/envoy_ndjson_to_sse.wasm`.

### Example Setup

1. **Backend**: A service that returns NDJSON (e.g. `Content-Type: application/x-ndjson`, body like `{"a":1}\n{"b":2}\n`).

2. **Envoy config** (example `envoy.yaml`):

```yaml
static_resources:
  listeners:
    - name: main
      address:
        socket_address: { address: 0.0.0.0, port_value: 10000 }
      filter_chains:
        - filters:
            - name: envoy.filters.network.http_connection_manager
              typed_config:
                "@type": type.googleapis.com/envoy.extensions.filters.network.http_connection_manager.v3.HttpConnectionManager
                stat_prefix: ingress
                route_config:
                  name: local_route
                  virtual_hosts:
                    - name: local
                      domains: ["*"]
                      routes:
                        - match: { prefix: "/" }
                          route: { cluster: backend }
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
                              filename: /path/to/envoy_ndjson_to_sse.wasm  # Path to WASM file
                  - name: envoy.filters.http.router
                    typed_config:
                      "@type": type.googleapis.com/envoy.extensions.filters.http.router.v3.Router
  clusters:
    - name: backend
      type: STRICT_DNS
      lb_policy: ROUND_ROBIN
      load_assignment:
        cluster_name: backend
        endpoints:
          - lb_endpoints:
              - endpoint:
                  address:
                    socket_address: { address: backend, port_value: 8080 }
```

3. **Run**  
   With Envoy and the backend running:

```bash
curl -N http://localhost:10000/your-ndjson-endpoint
```

- `-N`: Keeps the SSE stream open (no buffering).
- If the response has `Content-Type: text/event-stream` and body lines like `data: {"a":1}\n\n`, the filter is working.

### Docker Compose Example

You can run a simple backend that returns NDJSON and have Envoy load the WASM filter. Mount the WASM file so the Envoy container can read it, and use the same `envoy.yaml` as above.

---

**Summary**

| Method              | Command / Environment   | Purpose                          |
|---------------------|--------------------------|----------------------------------|
| Unit tests          | `cargo test`             | Validate NDJSON→SSE conversion   |
| Envoy integration   | Envoy + backend + curl   | Validate real WASM filter behavior |
