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

## Linux Setup

On Linux, a udev rule is required to grant non-root access to the USB device.

**1. Create the udev rule:**

```bash
sudo tee /etc/udev/rules.d/99-enody-esp32.rules <<'EOF'
SUBSYSTEM=="usb", ATTR{idVendor}=="303a", ATTR{idProduct}=="1001", MODE="0660", GROUP="plugdev", TAG+="uaccess"
EOF
```

**2. Add your user to the plugdev group (if not already):**

```bash
sudo usermod -aG plugdev $USER
```

**3. Reload udev rules:**

```bash
sudo udevadm control --reload-rules && sudo udevadm trigger
```

You may need to log out and back in for group changes to take effect.

### Troubleshooting

If `cargo run list` reports `USB(Access)`, verify permissions with:

```bash
# Check device permissions (should show group "plugdev" with rw access)
ls -la /dev/bus/usb/$(lsusb -d 303a:1001 | awk '{print $2"/"$4}' | tr -d :)

# Check your groups include plugdev
groups
```

Run with `RUST_LOG=debug` for detailed USB diagnostics:

```bash
RUST_LOG=debug cargo run list
```

If Ubuntu's `ModemManager` interferes with the device, add
`ENV{ID_MM_DEVICE_IGNORE}="1"` to the udev rule and reload.
