# SBA Lite

Lightweight, JVM-free monitoring server designed specifically for **Spring Boot applications** exposing **Actuator** endpoints.

SBA Lite embeds the official Spring Boot Admin Vue UI into a native Rust backend. Rather than acting as a simple network proxy, the Rust core provides an active **payload translation layer**: it intercepts, parses, and reformats remote Spring Boot Actuator responses in real time to match the exact data structures expected by the frontend Vue UI.<p >
  <img src="assets/screenshot_sba.png" alt="Runtime Memory Usage (RSS) Comparison Over Time: Java JVM vs GraalVM AOT vs SBA Lite Rust" width="1200">
</p>


## Features

- **Embedded UI**: The official Spring Boot Admin Vue UI is compiled directly into the single executable binary, requiring no external asset deployment.
- **Lightweight Backend**: Zero-JVM, low-overhead monitoring powered by a native Rust core using ~15 MB of memory.
- **Actuator Adaptation & Translation**: Actively intercepts and translates remote Spring Boot Actuator payloads to seamlessly match the expected data structures of the UI.
- **State & Discovery**: Application and instance discovery with continuous health polling and an event journal.
- **Real-time Updates**: Live instance state updates driven by Server-Sent Events (SSE).

## Memory Footprint & Runtime Behavior

Observed Resident Set Size (RSS) in the author's environment over a 180-second idle monitoring cycle:

| Implementation | Average Memory (RSS) | Memory Behavior Over Time |
|---|:---:|---|
| Spring Boot Admin — JVM | ~260.3 MB | **High fluctuation:** High initial peak during JIT compilation and class loading, followed by ongoing Garbage Collector cycles (jagged curve). |
| Spring Boot Admin — GraalVM AOT | ~112.1 MB | **Moderate stability:** Bypasses JIT overhead via Ahead-of-Time compilation, though memory slightly shifts during native GC routines. |
| **SBA Lite — Rust** | **~15.6 MB** | **Ultra-low consumption & absolute stability:** Minimal resource footprint with a rock-solid flat line. No runtime or VM overhead; memory is allocated and freed deterministically. |

### Benchmark Visualization

Below are the real-time telemetry charts captured during execution. The time-series graph highlights the difference in runtime dynamics, while the bar chart summarizes the overall resource distribution.

<p >
  <img src="assets/benchmark_time_series.png" alt="Runtime Memory Usage (RSS) Comparison Over Time: Java JVM vs GraalVM AOT vs SBA Lite Rust" width="850">
</p>

<p >
  <img src="assets/benchmark_summary_bars.png" alt="Average Memory Usage (RSS) Summary Bars: Java JVM 260.3 MB, GraalVM AOT 112.1 MB, SBA Lite Rust 15.6 MB" width="850">
</p>

*Note: These figures are environment-specific, captured on macOS, and are intended to showcase runtime resource distribution behavior rather than serve as a formal vendor benchmark.*

## Requirements

- Spring Boot applications with Spring Boot Actuator enabled.

Precompiled binaries are available for Linux (x86_64/aarch64), macOS (Intel/Apple Silicon), and Windows (x86_64).

## Quick Start

SBA Lite runs as a zero-dependency setup requiring only **two files**: the precompiled executable binary and the configuration file.

### 1. Configuration

Create a file named `instances.toml` in the same directory as your executable and add your configuration.

Example `instances.toml`:

```toml
[server]
port = 9001
poll_interval_secs = 10 # every quantum query remote actuators
insecure_tls = false # true ONLY local or no prod use, otherwise use proper CA certs (ca_cert_path)
ca_cert_path = ["/path/to/ca-a.pem", "/path/to/ca-b.pem"] # optional, if you want to use custom CA certs for HTTPS connections
log_level = "info" # "info" (default) | "debug" (log every remote call) | "trace" (+ health response body)
journal_max_events = 500 # circular buffer: beyond this number, events older than the journal are discarded

# if you set BOTH, protect UI and API with HTTP Basic Auth
#auth_username = "admin" 
#auth_password = "hello"

# list of monitored Spring Boot Actuator instances
[[instance]]
name = "my-service1"
actuator_base_url = "https://host1/context/actuator"
# bearer_token = "..."

[[instance]]
name = "my-service2"
actuator_base_url = "https://host2/context/actuator"
# bearer_token = "..."
```

### 2. Run

Launch the Rust server binary directly from your terminal:

```bash
./sbalite
```

Open `http://localhost:9001` in your browser.

## Spring Boot Actuator

The monitored application must expose Actuator over HTTP.

Maven:

```xml
<dependency>
    <groupId>org.springframework.boot</groupId>
    <artifactId>spring-boot-starter-actuator</artifactId>
</dependency>
```

Gradle:

```groovy
implementation 'org.springframework.boot:spring-boot-starter-actuator'
```

For full endpoint support, expose the endpoints required by your environment:

```yaml
management:
  endpoints:
    web:
      exposure:
        include: "*"
  endpoint:
    health:
      show-details: always
  info:
    env:
      enabled: true
```

For production, prefer an explicit endpoint list and protect sensitive Actuator endpoints with authentication.

Verify the remote Actuator:

```bash
curl -s https://host/context/actuator
curl -s https://host/context/actuator/health
```

## Building from source

```bash
cargo build --release
```

## Updating the embedded UI

The UI in `ui-dist/` is extracted from the official `spring-boot-admin-server-ui` jar (not built from source). To update to a newer Spring Boot Admin version:

1. Locate `spring-boot-admin-server-ui-<version>.jar` in your local Maven/Gradle cache.
2. Extract its static resources into `ui-dist/`.
3. Ensure `ui-dist/index.html` uses a relative `<base href="/" />`.

## Docker

To build the image locally, run the following command from the root directory of the project:

```bash
docker build -t sbalite-local .
```

To run the container with your custom configuration and certificates, execute:

```bash
docker run -d \
  --name my-sbalite \
  -p 9001:9001 \
  -v "\$(pwd)/instances.toml:/config/instances.toml" \
  -v "/PATH/TO/YOUR/LOCAL/CERTIFICATES:/etc/ssl/certs" \
  sbalite-local
```

### ℹ️ Configuration Notes
* **`instances.toml`**: Run the command from the folder containing your configuration file. `$(pwd)` automatically resolves to your current working directory.
* **Certificates**: Replace `/PATH/TO/YOUR/LOCAL/CERTIFICATES` with the absolute path to the directory on your host machine containing your custom CA certificates (e.g., `.pem` or `.crt` files). This allows the proxy to authenticate external HTTPS connections.
* **Subsequent Runs**: To stop the proxy, use `docker stop my-sbalite`. To start it again without re-creating the container, simply run `docker start my-sbalite`.

## License

SBA Lite is licensed under the [Apache License 2.0](LICENSE).

The embedded UI assets originate from [Spring Boot Admin](https://github.com/codecentric/spring-boot-admin), licensed under Apache License 2.0. See [NOTICE](NOTICE) for attribution.

SBA Lite is an independent, unofficial implementation of a Spring Boot Admin-compatible backend and is not affiliated with or endorsed by the Spring Boot Admin authors or Broadcom.
