# enody

A Rust library for interfacing with Enody spectrally tunable lighting devices. Supports both `std` host applications and `no_std` embedded firmware.

This library is very much WIP. I'm uploading to crates now as it is being distributed publicly and I want to reserve the name.

## Architecture

enody models a hierarchical device topology:

```
Environment -> Runtime -> Host -> Fixture -> Source -> Emitter
```

- **Environment**: Discovery surface that owns RemoteRuntimes (e.g. USB bus scan)
- **Runtime**: Message-passing entry point; each runtime has exactly one Host
- **Host**: Physical device (e.g. EP01) containing Fixtures
- **Fixture**: Addressable light output unit containing Sources
- **Source**: Independently controllable region within a Fixture, containing Emitters
- **Emitter**: Single LED channel with a spectral distribution

Each layer defines a trait for local/embedded use and a `Remote*` concrete type (behind the `remote` feature) for host-side USB communication. Remote types hold a cloned `RemoteRuntime` with a shared connection, so they can be freely passed around and used concurrently.

### Discovery and resource traversal

```rust
use enody::{environment::Environment, runtime::usb::USBEnvironment};

let environment = USBEnvironment::new();
for runtime in environment.runtimes() {
    let host = runtime.host().await?;
    let fixtures = host.fixtures().await?;
    for fixture in &fixtures {
        let sources = fixture.sources().await?;
        for source in &sources {
            let emitter_count = source.emitter_count().await?;
        }
    }
}
```

### Controlling fixtures

```rust
use enody::message::{Configuration, Flux};

// Set a fixture to 4000K blackbody at 80% brightness
fixture.display(
    Configuration::Blackbody(4000.0),
    Flux::Relative(0.8),
).await?;
```

## Features

- `std` (default via `remote`): Standard library support
- `remote` (default): USB communication via rusb/tokio. Enables `Environment`, `RemoteRuntime`, `RemoteHost`, `RemoteFixture`, `RemoteSource`, and the CLI binary
- `cli` (default): Command-line tool for device interaction

When all default features are disabled (`default-features = false`), the crate is `no_std` compatible for embedded use. The trait definitions and message types are always available.

## Usage

```toml
[dependencies]
enody = "0.1.0"                                    # remote + cli (default)
enody = { version = "0.1.0", default-features = false }  # no_std
```

## CLI

The included binary provides device management and control commands:

```
enody list                                  # List attached devices
enody info                                  # Device/fixture/source hierarchy
enody monitor                               # Stream device log output
enody set-blackbody 4000                    # Set all fixtures to 4000K
enody set-blackbody 6500 --flux 0.8         # 6500K at 80% brightness
enody strobe 4000 --rate 120 --duration 2   # Strobe at 120fps for 2s
enody fade --from-cct 6500 --to-cct 2700 --duration 5  # Linear CCT fade
```

## Build

Requires nightly Rust.

```bash
cargo build                           # Default features (remote + cli)
cargo build --no-default-features     # no_std
cargo run                             # Run CLI (device must be connected)
cargo test                            # Run tests
```
